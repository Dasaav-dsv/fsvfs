use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY};
use winfsp::{U16CStr, filesystem::{FileInfo, FileSecurity, FileSystemContext, OpenFileInfo}};
use winfsp_sys::FILE_ACCESS_RIGHTS;

pub struct FsContext;

impl FileSystemContext for FsContext {
    type FileContext = ();

    fn get_security_by_name(
        &self,
        _file_name: &U16CStr,
        _security_descriptor: Option<&mut [std::ffi::c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> winfsp::Result<FileSecurity>
    {
        let mut attributes = FILE_ATTRIBUTE_READONLY;

        if false {
            attributes |= FILE_ATTRIBUTE_DIRECTORY;
        }

        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0,
            attributes: attributes.0,
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext>
    {
        unimplemented!();
    }

    fn close(&self, context: Self::FileContext) {
        unimplemented!();
    }

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> winfsp::Result<()> {
        unimplemented!();
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        pattern: Option<&U16CStr>,
        marker: winfsp::filesystem::DirMarker,
        buffer: &mut [u8],
    ) -> winfsp::Result<u32>
    {
        unimplemented!();
    }

    fn read(&self, context: &Self::FileContext, buffer: &mut [u8], offset: u64) -> winfsp::Result<u32> {
        unimplemented!();
    }

    fn get_volume_info(&self, out_volume_info: &mut winfsp::filesystem::VolumeInfo) -> winfsp::Result<()> {
        unimplemented!();
    }
}
