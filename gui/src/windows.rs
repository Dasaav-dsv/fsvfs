use std::{ffi::c_void, os::windows::io::AsRawHandle, process::Child};

use windows::Win32::{
    Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectA, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_LIMIT_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Threading::GetCurrentProcess,
    },
};

#[derive(Debug)]
pub struct ChildKiller(HANDLE);

impl ChildKiller {
    pub fn new() -> eyre::Result<Self> {
        unsafe {
            let hjob = CreateJobObjectA(None, None)?;

            let information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK,
                    ..Default::default()
                },
                ..Default::default()
            };

            SetInformationJobObject(
                hjob,
                JobObjectExtendedLimitInformation,
                &raw const information as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;

            Ok(Self(hjob))
        }
    }

    pub fn kill_on_exit(&self, child: &Child) -> eyre::Result<()> {
        let process_handle = child.as_raw_handle();

        unsafe {
            AssignProcessToJobObject(self.0, HANDLE(process_handle))?;
        }

        Ok(())
    }
}

impl Clone for ChildKiller {
    fn clone(&self) -> Self {
        unsafe {
            let current_process = GetCurrentProcess();
            let mut cloned_handle = HANDLE::default();

            let res = DuplicateHandle(
                current_process,
                self.0,
                current_process,
                &mut cloned_handle,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            );

            if res.is_err() {
                cloned_handle = HANDLE::default();
            }

            Self(cloned_handle)
        }
    }
}

impl Drop for ChildKiller {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

unsafe impl Send for ChildKiller {}

unsafe impl Sync for ChildKiller {}
