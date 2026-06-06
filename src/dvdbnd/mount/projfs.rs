use std::{
    alloc::{Layout, alloc, dealloc},
    any::type_name_of_val,
    ffi::c_void,
    fs,
    io::Write,
    mem,
    num::NonZero,
    ops::Range,
    os::windows::fs::OpenOptionsExt,
    panic::{self, AssertUnwindSafe},
    ptr,
    sync::{
        Arc, Weak,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
    time::Duration,
};

use color_eyre::eyre::{self, eyre};
use fxhash::FxBuildHasher;
use papaya::HashMap as PapayaMap;
use parking_lot::RwLock;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::{
            E_ABORT, E_FAIL, E_INVALIDARG, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND,
            ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_OPERATION, ERROR_OFFSET_ALIGNMENT_VIOLATION,
            S_OK,
        },
        Storage::{
            FileSystem::FILE_ATTRIBUTE_HIDDEN,
            ProjectedFileSystem::{
                PRJ_CALLBACK_DATA, PRJ_CALLBACKS, PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN,
                PRJ_DIR_ENTRY_BUFFER_HANDLE, PRJ_FILE_BASIC_INFO, PRJ_FLAG_USE_NEGATIVE_PATH_CACHE,
                PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT, PRJ_NOTIFICATION, PRJ_NOTIFICATION_MAPPING,
                PRJ_NOTIFICATION_PARAMETERS, PRJ_NOTIFICATION_PRE_DELETE,
                PRJ_NOTIFICATION_PRE_RENAME, PRJ_NOTIFY_PRE_DELETE, PRJ_NOTIFY_PRE_RENAME,
                PRJ_PLACEHOLDER_INFO, PRJ_STARTVIRTUALIZING_OPTIONS,
                PRJ_VIRTUALIZATION_INSTANCE_INFO, PrjFileNameCompare, PrjFileNameMatch,
                PrjFillDirEntryBuffer, PrjGetVirtualizationInstanceInfo,
                PrjMarkDirectoryAsPlaceholder, PrjStartVirtualizing, PrjStopVirtualizing,
                PrjWriteFileData, PrjWritePlaceholderInfo,
            },
        },
    },
    core::{GUID, HRESULT, HSTRING, PCWSTR, Result as WindowsResult, w},
};

use crate::{
    dvdbnd::{
        filesystem::DvdbndFilesystem,
        mount::{DvdbndFile, DvdbndMount},
    },
    filesystem::readonly::{Entry, ReadOnlyFilesystem, RofsError},
    runas::runas_powershell_command,
    thread::{OnInterrupt, run_until_interrupted},
};

#[derive(Debug)]
struct MountContext<F: DvdbndFilesystem> {
    mount: DvdbndMount<F>,
    enumerations: PapayaMap<GUID, Arc<DirEnumeration>, FxBuildHasher>,
    context: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    instance_alignment: RwLock<Option<NonZero<u32>>>,
    mountpoint: HSTRING,
    weak_ptr: AtomicPtr<Self>,
}

#[derive(Debug)]
struct DirEnumeration {
    pos: AtomicUsize,
    index: Box<[(usize, u64)]>,
    store: Box<[u16]>,
}

const E_BUFFER: HRESULT = ERROR_INSUFFICIENT_BUFFER.to_hresult();

impl<F> DvdbndMount<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    pub fn mount(self, mountpoint: &str) -> eyre::Result<()> {
        require_projfs()?;

        let mountpoint = init_mountpoint(mountpoint)?;

        info!("mounting {mountpoint}");

        let mount_context = MountContext::new(self, mountpoint);
        let mount = mount_context.mount()?;

        run_until_interrupted(mount)?;

        Ok(())
    }

    fn file_basic_info<T: DvdbndFile>(&self, entry: &Entry<'_, T>) -> PRJ_FILE_BASIC_INFO {
        let (is_dir, file_size) = match entry {
            Entry::Dir(_) => (true, 0),
            Entry::File(file) => (false, file.len()),
            Entry::Link(_) => (false, 0),
        };

        PRJ_FILE_BASIC_INFO {
            IsDirectory: is_dir,
            FileSize: file_size.into(),
            ..Default::default()
        }
    }
}

impl<F> MountContext<F>
where
    F: DvdbndFilesystem + Send + Sync + 'static,
{
    fn new(mount: DvdbndMount<F>, mountpoint: HSTRING) -> Self {
        Self {
            mount,
            enumerations: PapayaMap::default(),
            context: Default::default(),
            instance_alignment: RwLock::new(None),
            mountpoint,
            weak_ptr: AtomicPtr::new(Weak::<Self>::new().as_ptr() as *mut Self),
        }
    }

    fn mount(self) -> eyre::Result<Arc<Self>> {
        let mut mount_context = Arc::new(self);

        let thread_count = match std::thread::available_parallelism() {
            Ok(threads) => threads.get().min(16) as u32,
            Err(_) => 0,
        };

        let mut notifications = PRJ_NOTIFICATION_MAPPING {
            NotificationBitMask: PRJ_NOTIFY_PRE_DELETE | PRJ_NOTIFY_PRE_RENAME,
            NotificationRoot: w!(""),
        };

        let options = PRJ_STARTVIRTUALIZING_OPTIONS {
            Flags: PRJ_FLAG_USE_NEGATIVE_PATH_CACHE,
            PoolThreadCount: thread_count,
            ConcurrentThreadCount: thread_count,
            NotificationMappings: &mut notifications,
            NotificationMappingsCount: 1,
        };

        let context_out = &raw mut Arc::get_mut(&mut mount_context).unwrap().context;

        let weak = Arc::downgrade(&mount_context);
        let context_in = Weak::as_ptr(&weak);

        unsafe {
            *context_out = PrjStartVirtualizing(
                &mount_context.mountpoint,
                &MountContext::<F>::CALLBACKS,
                Some(context_in as *const c_void),
                Some(&options),
            )?;
        }

        mount_context
            .weak_ptr
            .store(context_in as *mut Self, Ordering::Relaxed);

        mem::forget(weak);

        Ok(mount_context)
    }

    fn instance_alignment(&self) -> WindowsResult<NonZero<u32>> {
        if let Some(alignment) = &*self.instance_alignment.read() {
            return Ok(*alignment);
        }

        let mut info = PRJ_VIRTUALIZATION_INSTANCE_INFO::default();

        unsafe {
            PrjGetVirtualizationInstanceInfo(self.context, &mut info)?;
        }

        let Some(alignment) = NonZero::new(info.WriteAlignment) else {
            return Err(ERROR_OFFSET_ALIGNMENT_VIOLATION.into());
        };

        *self.instance_alignment.write() = Some(alignment);

        Ok(alignment)
    }

    unsafe extern "system" fn start_directory_enumeration(
        callbackdata: &PRJ_CALLBACK_DATA,
        enumerationid: &GUID,
    ) -> HRESULT {
        Self::call(callbackdata, |context| {
            let fs = context.mount.fs.as_rofs();
            let path = unsafe { callbackdata.FilePathName.to_string()? };

            let Ok(inode) = fs.lookup([path.as_str()]) else {
                return Err(ERROR_FILE_NOT_FOUND.into());
            };

            let Ok(Entry::Dir(inode_range)) = fs.entry(inode, false) else {
                return Err(ERROR_DIRECTORY.into());
            };

            let Ok(enumeration) = DirEnumeration::from_inode_range(inode_range, fs) else {
                return Err(ERROR_DIRECTORY.into());
            };

            let guard = context.enumerations.guard();
            context
                .enumerations
                .insert(*enumerationid, Arc::new(enumeration), &guard);

            Ok(())
        })
    }

    unsafe extern "system" fn end_directory_enumeration(
        callbackdata: &PRJ_CALLBACK_DATA,
        enumerationid: &GUID,
    ) -> HRESULT {
        Self::call(callbackdata, |context| {
            let guard = context.enumerations.guard();
            context.enumerations.remove(enumerationid, &guard);

            Ok(())
        })
    }

    unsafe extern "system" fn get_directory_enumeration(
        callbackdata: &PRJ_CALLBACK_DATA,
        enumerationid: &GUID,
        searchexpression: PCWSTR,
        direntrybufferhandle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
    ) -> HRESULT {
        Self::call(callbackdata, |context| {
            let dir = {
                let guard = context.enumerations.guard();
                let Some(enumeration) = context.enumerations.get(enumerationid, &guard) else {
                    return Err(E_INVALIDARG.into());
                };
                enumeration.clone()
            };

            if callbackdata.Flags.0 & PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN.0 != 0 {
                dir.reset();
            }

            let fs = context.mount.fs.as_rofs();

            let start_pos = dir.pos();
            let mut end_pos = start_pos;

            for (pos, (path, inode)) in dir.enumerate(searchexpression) {
                let Ok(entry) = fs.entry(inode, true) else {
                    return Err(E_FAIL.into());
                };

                let res = unsafe {
                    PrjFillDirEntryBuffer(
                        path,
                        Some(&context.mount.file_basic_info(&entry)),
                        direntrybufferhandle,
                    )
                };

                if let Err(e) = res {
                    match e.code() {
                        E_BUFFER if start_pos != end_pos => break,
                        _ => return Err(e),
                    }
                }

                end_pos = pos + 1;
            }

            dir.set_pos(end_pos);

            Ok(())
        })
    }

    unsafe extern "system" fn get_placeholder_info(callbackdata: &PRJ_CALLBACK_DATA) -> HRESULT {
        Self::call(callbackdata, |context| {
            let fs = context.mount.fs.as_rofs();
            let path = unsafe { callbackdata.FilePathName.to_string()? };

            let Ok(inode) = fs.lookup([path.as_str()]) else {
                return Err(ERROR_FILE_NOT_FOUND.into());
            };

            let Ok(entry) = fs.entry(inode, true) else {
                return Err(E_FAIL.into());
            };

            let placeholder_info = PRJ_PLACEHOLDER_INFO {
                FileBasicInfo: context.mount.file_basic_info(&entry),
                ..Default::default()
            };

            unsafe {
                PrjWritePlaceholderInfo(
                    callbackdata.NamespaceVirtualizationContext,
                    callbackdata.FilePathName,
                    &placeholder_info,
                    size_of::<PRJ_PLACEHOLDER_INFO>() as u32,
                )
            }
        })
    }

    unsafe extern "system" fn get_file_data(
        callbackdata: &PRJ_CALLBACK_DATA,
        mut byteoffset: u64,
        mut length: u32,
    ) -> HRESULT {
        Self::call(callbackdata, |context| {
            let fs = context.mount.fs.as_rofs();
            let path = unsafe { callbackdata.FilePathName.to_string()? };

            let Ok(inode) = fs.lookup([path.as_str()]) else {
                return Err(ERROR_FILE_NOT_FOUND.into());
            };

            let mut reader = match context.mount.make_reader(inode) {
                Ok(reader) => reader,
                Err(e) => {
                    warn!("could not open file: {e}");
                    return Err(E_FAIL.into());
                }
            };

            let alignment = context.instance_alignment()?.get();

            let mut bytes = context.mount.read_mut(&mut reader, length);

            let bytes_addr = bytes.as_ptr().addr();
            let aligned_addr = bytes_addr.next_multiple_of(alignment as usize);

            let unaligned_len = (aligned_addr - bytes_addr).min(bytes.len());
            let aligned_buffer_len = unaligned_len
                .next_multiple_of(alignment as usize)
                .min(bytes.len());

            if unaligned_len != 0 {
                let unaligned_bytes = &bytes[..aligned_buffer_len];
                bytes = &bytes[unaligned_len..];

                let layout = Layout::from_size_align(aligned_buffer_len, alignment as usize)
                    .expect("bad alignment parameters");

                let aligned_buffer = unsafe { alloc(layout) };

                unsafe {
                    ptr::copy_nonoverlapping(
                        unaligned_bytes.as_ptr(),
                        aligned_buffer,
                        aligned_buffer_len,
                    );
                }

                let res = unsafe {
                    PrjWriteFileData(
                        callbackdata.NamespaceVirtualizationContext,
                        &callbackdata.DataStreamId,
                        aligned_buffer as *const c_void,
                        byteoffset,
                        aligned_buffer_len as u32,
                    )
                };

                unsafe {
                    dealloc(aligned_buffer, layout);
                }

                byteoffset += unaligned_len as u64;
                length -= unaligned_len as u32;

                res?;
            }

            let file_len = reader.len as u64;

            if file_len > byteoffset + length as u64 {
                let max_len = (file_len - byteoffset) as u32;
                length = length.next_multiple_of(alignment).min(max_len);
            }

            if length != 0 && !bytes.is_empty() {
                unsafe {
                    PrjWriteFileData(
                        callbackdata.NamespaceVirtualizationContext,
                        &callbackdata.DataStreamId,
                        bytes.as_ptr() as *const c_void,
                        byteoffset,
                        length,
                    )?
                };
            }

            Ok(())
        })
    }

    unsafe extern "system" fn notify(
        _callbackdata: &PRJ_CALLBACK_DATA,
        _isdirectory: bool,
        notification: PRJ_NOTIFICATION,
        _destinationfilename: PCWSTR,
        _operationparameters: &mut PRJ_NOTIFICATION_PARAMETERS,
    ) -> HRESULT {
        match notification {
            PRJ_NOTIFICATION_PRE_DELETE | PRJ_NOTIFICATION_PRE_RENAME => {
                ERROR_INVALID_OPERATION.to_hresult()
            }
            _ => S_OK,
        }
    }

    #[track_caller]
    fn call(
        callbackdata: &PRJ_CALLBACK_DATA,
        f: impl FnOnce(&Self) -> WindowsResult<()>,
    ) -> HRESULT {
        let Some(context) =
            (unsafe { Weak::from_raw(callbackdata.InstanceContext as *const Self).upgrade() })
        else {
            return E_ABORT;
        };

        let name = type_name_of_val(&f);
        let res = match panic::catch_unwind(AssertUnwindSafe(|| f(&context))) {
            Ok(res) => match res {
                Ok(_) => S_OK,
                Err(e) => {
                    warn!("callback returned an error: {e}\n\tat {name}");
                    e.code()
                }
            },
            Err(payload) => {
                if let Some(&payload) = payload.downcast_ref::<&'static str>() {
                    warn!("callback panicked: {payload}\n\tat {name}");
                }
                E_FAIL
            }
        };

        mem::forget(Arc::downgrade(&context));

        res
    }

    const CALLBACKS: PRJ_CALLBACKS = unsafe {
        PRJ_CALLBACKS {
            StartDirectoryEnumerationCallback: Some(mem::transmute::<
                unsafe extern "system" fn(&PRJ_CALLBACK_DATA, &GUID) -> HRESULT,
                unsafe extern "system" fn(*const PRJ_CALLBACK_DATA, *const GUID) -> HRESULT,
            >(
                Self::start_directory_enumeration
            )),

            EndDirectoryEnumerationCallback: Some(mem::transmute::<
                unsafe extern "system" fn(&PRJ_CALLBACK_DATA, &GUID) -> HRESULT,
                unsafe extern "system" fn(*const PRJ_CALLBACK_DATA, *const GUID) -> HRESULT,
            >(Self::end_directory_enumeration)),

            GetDirectoryEnumerationCallback: Some(mem::transmute::<
                unsafe extern "system" fn(
                    &PRJ_CALLBACK_DATA,
                    &GUID,
                    PCWSTR,
                    PRJ_DIR_ENTRY_BUFFER_HANDLE,
                ) -> HRESULT,
                unsafe extern "system" fn(
                    *const PRJ_CALLBACK_DATA,
                    *const GUID,
                    PCWSTR,
                    PRJ_DIR_ENTRY_BUFFER_HANDLE,
                ) -> HRESULT,
            >(Self::get_directory_enumeration)),

            GetPlaceholderInfoCallback: Some(mem::transmute::<
                unsafe extern "system" fn(&PRJ_CALLBACK_DATA) -> HRESULT,
                unsafe extern "system" fn(*const PRJ_CALLBACK_DATA) -> HRESULT,
            >(Self::get_placeholder_info)),

            GetFileDataCallback: Some(mem::transmute::<
                unsafe extern "system" fn(&PRJ_CALLBACK_DATA, u64, u32) -> HRESULT,
                unsafe extern "system" fn(*const PRJ_CALLBACK_DATA, u64, u32) -> HRESULT,
            >(Self::get_file_data)),

            NotificationCallback: Some(mem::transmute::<
                unsafe extern "system" fn(
                    &PRJ_CALLBACK_DATA,
                    bool,
                    PRJ_NOTIFICATION,
                    PCWSTR,
                    &mut PRJ_NOTIFICATION_PARAMETERS,
                ) -> HRESULT,
                unsafe extern "system" fn(
                    *const PRJ_CALLBACK_DATA,
                    bool,
                    PRJ_NOTIFICATION,
                    PCWSTR,
                    *mut PRJ_NOTIFICATION_PARAMETERS,
                ) -> HRESULT,
            >(Self::notify)),

            QueryFileNameCallback: None,

            CancelCommandCallback: None,
        }
    };
}

impl DirEnumeration {
    fn from_inode_range<F: ReadOnlyFilesystem>(
        inodes: Range<u64>,
        fs: &F,
    ) -> Result<Self, RofsError> {
        let len = inodes.clone().count();

        let mut index = Vec::with_capacity(len);
        let mut store = Vec::with_capacity(len * 32);

        for inode in inodes {
            let path = fs.path(inode)?;

            let name = match path.rsplit_once('/') {
                Some((_, name)) => name,
                None => path,
            };

            index.push((store.len(), inode));
            store.extend(name.encode_utf16().chain([0]));
        }

        index.sort_unstable_by(|(a, _), (b, _)| unsafe {
            let a = PCWSTR(store[*a..].as_ptr());
            let b = PCWSTR(store[*b..].as_ptr());
            PrjFileNameCompare(a, b).cmp(&0)
        });

        Ok(Self {
            pos: AtomicUsize::new(0),
            index: index.into_boxed_slice(),
            store: store.into_boxed_slice(),
        })
    }

    fn enumerate(&self, expression: PCWSTR) -> impl Iterator<Item = (usize, (PCWSTR, u64))> {
        let pos = self.pos();

        self.index
            .iter()
            .enumerate()
            .skip(pos)
            .filter_map(move |(pos, (path_index, inode))| {
                let path = PCWSTR(self.store[*path_index..].as_ptr());

                if expression.is_null() || unsafe { PrjFileNameMatch(path, expression) } {
                    Some((pos, (path, *inode)))
                } else {
                    None
                }
            })
    }

    fn pos(&self) -> usize {
        self.pos.load(Ordering::Acquire)
    }

    fn set_pos(&self, new_pos: usize) {
        let pos = new_pos.min(self.index.len());
        self.pos.store(pos, Ordering::Release);
    }

    fn reset(&self) {
        self.pos.store(0, Ordering::Release);
    }
}

impl<F: DvdbndFilesystem> OnInterrupt for Arc<MountContext<F>> {
    type Error = eyre::Error;

    fn on_interrupt(self) -> Result<(), Self::Error> {
        let mountpoint = self.mountpoint.to_string();
        info!("unmounting {mountpoint}");

        unsafe {
            PrjStopVirtualizing(self.context);

            let weak_ptr = self.weak_ptr.load(Ordering::SeqCst);
            drop(Weak::from_raw(weak_ptr));
        }

        let mut is_waiting = false;
        loop {
            let Err(e) = fs::remove_dir_all(mountpoint.as_str()) else {
                break;
            };

            // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION or a virtualization error.
            if !matches!(e.raw_os_error(), Some(32) | Some(33) | Some(369)) {
                return Err(e.into());
            }

            if is_waiting {
                info!("waiting to shutdown, make sure to close all files and directories!");
            }

            std::thread::sleep(Duration::from_secs(3));

            is_waiting = true;
        }

        fs::create_dir(mountpoint)?;

        Ok(())
    }
}

fn require_projfs() -> eyre::Result<()> {
    info!("ensuring ProjFS is enabled...");

    let has_projfs = runas_powershell_command(
        "if ((Get-WindowsOptionalFeature -Online -FeatureName Client-ProjFS).State -eq 'disabled') { exit 1 }",
    )?;

    if !has_projfs.success() {
        warn!("attempting to enable ProjFS (it is either disabled or an error has occurred)");

        let enable_projfs = runas_powershell_command(
            "exit (Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS -NoRestart).ExitCode",
        )?.success();

        if !enable_projfs {
            return Err(eyre!(
                "failed to enable ProjFS! See https://learn.microsoft.com/en-us/windows/win32/projfs/enabling-windows-projected-file-system"
            ));
        }
    }

    Ok(())
}

fn init_mountpoint(mountpoint: &str) -> eyre::Result<HSTRING> {
    if fs::read_dir(mountpoint)?.count() != 0 {
        return Err(eyre!(
            "the filesystem must be mounted in an empty directory"
        ));
    }

    let guid = get_or_create_mount_guid(mountpoint)?;

    let mountpoint = HSTRING::from(mountpoint);
    unsafe {
        PrjMarkDirectoryAsPlaceholder(&mountpoint, None, None, &guid)?;
    }

    Ok(mountpoint)
}

fn get_or_create_mount_guid(mountpoint: &str) -> eyre::Result<GUID> {
    let path = format!("{mountpoint}.guid");

    if let Ok(bytes) = fs::read(&path)
        && let Some(bytes) = bytes.as_array()
    {
        let guid = GUID::from_u128(u128::from_le_bytes(*bytes));
        info!("read mountpoint GUID: {guid:?}");

        return Ok(guid);
    }

    let _ = fs::remove_file(&path);
    let guid = GUID::new()?;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN.0)
        .open(&path)?;

    file.write_all(&guid.to_u128().to_le_bytes())?;
    file.flush()?;

    info!("created mountpoint GUID: {guid:?}");

    Ok(guid)
}

unsafe impl<F: DvdbndFilesystem> Send for MountContext<F> {}

unsafe impl<F: DvdbndFilesystem> Sync for MountContext<F> {}
