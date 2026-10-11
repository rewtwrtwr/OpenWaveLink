
use std::os::windows::io::AsRawHandle;
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, SetInformationJobObject, AssignProcessToJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::Foundation::HANDLE;

fn main() {
    let child = std::process::Command::new("cmd.exe").spawn().unwrap();
    unsafe {
        let job = CreateJobObjectW(None, None).unwrap();
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ).unwrap();
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as *mut _)).unwrap();
    }
}

