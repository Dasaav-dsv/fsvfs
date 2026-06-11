use std::{
    any::type_name_of_val,
    ffi::c_void,
    fs,
    io::Write,
    mem::{self, ManuallyDrop},
    ops::Range,
    os::windows::fs::OpenOptionsExt,
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc, Weak,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
    time::Duration,
};

use color_eyre::eyre::{self, eyre};
use futures_util::{FutureExt, TryFutureExt};
use fxhash::FxBuildHasher;
use papaya::HashMap as PapayaMap;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::{
            E_ABORT, E_FAIL, E_INVALIDARG, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND,
            ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, S_OK,
        },
        Storage::{
            FileSystem::FILE_ATTRIBUTE_HIDDEN,
            ProjectedFileSystem::{
                PRJ_CALLBACK_DATA, PRJ_CALLBACKS, PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN,
                PRJ_DIR_ENTRY_BUFFER_HANDLE, PRJ_FILE_BASIC_INFO, PRJ_FLAG_USE_NEGATIVE_PATH_CACHE,
                PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT, PRJ_NOTIFICATION_MAPPING, PRJ_NOTIFY_NONE,
                PRJ_PLACEHOLDER_INFO, PRJ_STARTVIRTUALIZING_OPTIONS, PrjCompleteCommand,
                PrjFileNameCompare, PrjFileNameMatch, PrjFillDirEntryBuffer,
                PrjMarkDirectoryAsPlaceholder, PrjStartVirtualizing, PrjStopVirtualizing,
                PrjWriteFileData, PrjWritePlaceholderInfo,
            },
        },
        System::LibraryLoader::LoadLibraryW,
    },
    core::{Error as WindowsError, GUID, HRESULT, HSTRING, PCWSTR, Result as WindowsResult, w},
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
const E_PENDING: HRESULT = ERROR_IO_PENDING.to_hresult();

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
            Entry::File(file) => (false, file.unpadded_len()),
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
            mountpoint,
            weak_ptr: AtomicPtr::new(Weak::<Self>::new().as_ptr() as *mut Self),
        }
    }

    fn mount(self) -> eyre::Result<Arc<Self>> {
        let mut mount_context = Arc::new(self);

        const THREAD_COUNT: u32 = 4;

        let mut notifications = PRJ_NOTIFICATION_MAPPING {
            NotificationBitMask: PRJ_NOTIFY_NONE,
            NotificationRoot: w!(""),
        };

        let options = PRJ_STARTVIRTUALIZING_OPTIONS {
            Flags: PRJ_FLAG_USE_NEGATIVE_PATH_CACHE,
            PoolThreadCount: THREAD_COUNT,
            ConcurrentThreadCount: THREAD_COUNT,
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
        byteoffset: u64,
        length: u32,
    ) -> HRESULT {
        Self::call(callbackdata, |context| {
            let path = unsafe { callbackdata.FilePathName.to_string()? };

            let Ok(inode) = context.mount.fs.as_rofs().lookup([path.as_str()]) else {
                return Err(ERROR_FILE_NOT_FOUND.into());
            };

            let stream_id = callbackdata.DataStreamId;
            let dispatch = {
                let context = context.clone();
                move |buf: &[u8], file_offset: u32| {
                    unsafe {
                        PrjWriteFileData(
                            context.context,
                            &stream_id,
                            buf.as_ptr() as *const c_void,
                            file_offset as u64,
                            buf.len() as u32,
                        )?;
                    }

                    Ok(())
                }
            };

            Self::call_async(callbackdata, async move |context| {
                context
                    .mount
                    .read_file(inode, byteoffset, length, dispatch)
                    .map_err(|e| WindowsError::new(E_FAIL, e.to_string()))
                    .await
            })
            .ok()
        })
    }

    unsafe extern "system" fn cancel_command(_callbackdata: &PRJ_CALLBACK_DATA) {}

    #[inline(always)]
    fn call(
        callbackdata: &PRJ_CALLBACK_DATA,
        f: impl FnOnce(&Arc<Self>) -> WindowsResult<()>,
    ) -> HRESULT {
        let Some(context) = (unsafe {
            ManuallyDrop::new(Weak::from_raw(callbackdata.InstanceContext as *const Self)).upgrade()
        }) else {
            return E_ABORT;
        };

        let name = type_name_of_val(&f);

        match panic::catch_unwind(AssertUnwindSafe(|| f(&context))) {
            Ok(res) => match res {
                Ok(_) => S_OK,
                Err(e) => {
                    let code = e.code();
                    if code != E_PENDING {
                        warn!("callback returned an error: {e}\n\tat {name}");
                    }
                    code
                }
            },
            Err(payload) => {
                if let Some(&payload) = payload.downcast_ref::<&'static str>() {
                    warn!("callback panicked: {payload}\n\tat {name}");
                }
                E_FAIL
            }
        }
    }

    #[inline(always)]
    fn call_async<Fut>(
        callbackdata: &PRJ_CALLBACK_DATA,
        f: impl (FnOnce(Arc<Self>) -> Fut) + Send + 'static,
    ) -> HRESULT
    where
        Fut: Future<Output = WindowsResult<()>> + 'static,
    {
        let name = type_name_of_val(&f);
        let command_id = callbackdata.CommandId;

        let dispatch = |context: &Arc<Self>| {
            let complete = {
                let context = context.clone();
                async move || {
                    let res = AssertUnwindSafe(f(context.clone())).catch_unwind().await;

                    let res = match res {
                        Ok(res) => match res {
                            Ok(_) => S_OK,
                            Err(e) => {
                                warn!("async callback returned an error: {e}\n\tat {name}");
                                e.code()
                            }
                        },
                        Err(payload) => {
                            if let Some(&payload) = payload.downcast_ref::<&'static str>() {
                                warn!("async callback panicked: {payload}\n\tat {name}");
                            }
                            E_FAIL
                        }
                    };

                    if let Err(e) =
                        unsafe { PrjCompleteCommand(context.context, command_id, res, None) }
                    {
                        warn!("PrjCompleteCommand returned an error: {e}\n\tat {name}");
                    }
                }
            };

            let _ = context.mount.dispatcher.dispatch(complete);

            Err(E_PENDING.into())
        };

        Self::call(callbackdata, dispatch)
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

            CancelCommandCallback: Some(mem::transmute::<
                unsafe extern "system" fn(&PRJ_CALLBACK_DATA),
                unsafe extern "system" fn(*const PRJ_CALLBACK_DATA),
            >(Self::cancel_command)),

            NotificationCallback: None,

            QueryFileNameCallback: None,
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

        while let Err(e) = fs::remove_dir_all(mountpoint.as_str()) {
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

    let has_profjs_dll = unsafe { LoadLibraryW(w!("projectedfslib.dll")).is_ok() };

    if has_profjs_dll {
        info!(
            "loaded ProjFS DLL; if ProjFS is not actually enabled, see https://learn.microsoft.com/en-us/windows/win32/projfs/enabling-windows-projected-file-system"
        );
    }

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
    let path = match mountpoint.rsplit_once(['\\', '/']) {
        Some((parent, name)) => format!("{parent}/~{name}.GUID"),
        None => format!("~{mountpoint}.GUID"),
    };

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
