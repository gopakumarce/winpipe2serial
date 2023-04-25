use std::ffi::OsStr;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use std::env;
use std::io::Write;
use std::thread;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{BOOL, DWORD, FALSE, TRUE};
use winapi::shared::ntdef::NULL;
use winapi::shared::winerror::ERROR_IO_PENDING;
use winapi::shared::winerror::ERROR_PIPE_BUSY;
use winapi::um::commapi::{SetCommState, SetCommTimeouts};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::fileapi::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
use winapi::um::handleapi::{CloseHandle, DuplicateHandle, INVALID_HANDLE_VALUE};
use winapi::um::ioapiset::GetOverlappedResult;
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::namedpipeapi::{SetNamedPipeHandleState, WaitNamedPipeW};
use winapi::um::processthreadsapi::GetCurrentProcess;
use winapi::um::synchapi::CreateEventW;
use winapi::um::winbase::{
    CBR_115200, COMMTIMEOUTS, DCB, FILE_FLAG_OVERLAPPED, NOPARITY, ONESTOPBIT,
};
use winapi::um::winbase::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH};
use winapi::um::winbase::{PIPE_READMODE_BYTE, PIPE_WAIT};
use winapi::um::winnt::{DUPLICATE_SAME_ACCESS, GENERIC_READ, GENERIC_WRITE, HANDLE};

enum WhichHandle {
    Pipe,
    Serial,
}

pub struct Pipe2Serial {
    comdev: HANDLE,
    comevent: HANDLE,
    pipedev: HANDLE,
    pipeevent: HANDLE,
}

// Windows HANDLEs can be sent across threads
unsafe impl Send for Pipe2Serial {}
unsafe impl Sync for Pipe2Serial {}

impl Pipe2Serial {
    pub fn open(port_name: &str, pipe_name: &str) -> io::Result<Self> {
        let mut port_name_utf16 = Vec::<u16>::new();
        port_name_utf16.extend(OsStr::new("\\\\.\\").encode_wide());
        port_name_utf16.extend(OsStr::new(port_name).encode_wide());
        port_name_utf16.push(0);

        let comdev = unsafe {
            CreateFileW(
                port_name_utf16.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                ptr::null_mut(),
            )
        };
        if comdev == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let comevent = unsafe { CreateEventW(ptr::null_mut(), FALSE, FALSE, ptr::null_mut()) };
        if comevent == NULL {
            _ = unsafe { CloseHandle(comdev) };
            return Err(io::Error::last_os_error());
        }
        let mut dcb: DCB = unsafe { mem::zeroed() };
        dcb.DCBlength = mem::size_of::<DCB>() as u32;
        dcb.set_fBinary(TRUE as u32);
        dcb.BaudRate = CBR_115200;
        dcb.ByteSize = 8;
        dcb.StopBits = ONESTOPBIT;
        dcb.Parity = NOPARITY;
        if unsafe { SetCommState(comdev, &mut dcb) } == FALSE {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            return Err(io::Error::last_os_error());
        }

        // What on earth is this microsoft !? One needs to read the doc a dozen times
        // to understand what the hell this means. Right now the setting of "1" below
        // means that if we get one byte, wait one more millisecond for the next byte
        // and if the next byte doesnt come in the next 1msec just return that byte.
        // I dont need any wait-for-next-byte, so ideally I would expect a zero as the
        // setting for that, but no, zero means "wait indefinitely". Wierd
        let mut timeouts = COMMTIMEOUTS {
            ReadIntervalTimeout: 1,
            ReadTotalTimeoutMultiplier: 0,
            ReadTotalTimeoutConstant: 0,
            WriteTotalTimeoutMultiplier: 0,
            WriteTotalTimeoutConstant: 0,
        };
        if unsafe { SetCommTimeouts(comdev, &mut timeouts) } == FALSE {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            return Err(io::Error::last_os_error());
        }

        let mut pipe_name_utf16 = Vec::<u16>::new();
        pipe_name_utf16.extend(OsStr::new("\\\\.\\pipe\\").encode_wide());
        pipe_name_utf16.extend(OsStr::new(pipe_name).encode_wide());
        pipe_name_utf16.push(0);

        let pipedev = loop {
            let pipedev = unsafe {
                CreateFileW(
                    pipe_name_utf16.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null_mut(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                    ptr::null_mut(),
                )
            };
            if pipedev != INVALID_HANDLE_VALUE {
                break pipedev;
            }

            let err = unsafe { GetLastError() };
            if err != ERROR_PIPE_BUSY {
                println!("Could not open pipe, error {err:}");
                break INVALID_HANDLE_VALUE;
            }

            if unsafe { WaitNamedPipeW(pipe_name_utf16.as_ptr(), 20000) } == FALSE {
                let err = unsafe { GetLastError() };
                println!("Could not open pipe: 20 second wait timed out, {err:}");
                break INVALID_HANDLE_VALUE;
            }
        };
        if pipedev == INVALID_HANDLE_VALUE {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            return Err(io::Error::last_os_error());
        }
        let pipeevent = unsafe { CreateEventW(ptr::null_mut(), FALSE, FALSE, ptr::null_mut()) };
        if pipeevent == NULL {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            _ = unsafe { CloseHandle(pipedev) };
            return Err(io::Error::last_os_error());
        }
        let mut dw_mode: DWORD = PIPE_READMODE_BYTE | PIPE_WAIT;
        let res = unsafe {
            SetNamedPipeHandleState(pipedev, &mut dw_mode, ptr::null_mut(), ptr::null_mut())
        };
        if res == FALSE {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            _ = unsafe { CloseHandle(pipedev) };
            _ = unsafe { CloseHandle(pipeevent) };
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            comdev,
            comevent,
            pipedev,
            pipeevent,
        })
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        let mut comdev = INVALID_HANDLE_VALUE;
        let process = unsafe { GetCurrentProcess() };
        let res = unsafe {
            DuplicateHandle(
                process,
                self.comdev,
                process,
                &mut comdev,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if res == FALSE {
            return Err(io::Error::last_os_error());
        }
        let comevent = unsafe { CreateEventW(ptr::null_mut(), FALSE, FALSE, ptr::null_mut()) };
        if comevent == NULL {
            _ = unsafe { CloseHandle(comdev) };
            return Err(io::Error::last_os_error());
        }

        let mut pipedev = INVALID_HANDLE_VALUE;
        let process = unsafe { GetCurrentProcess() };
        let res = unsafe {
            DuplicateHandle(
                process,
                self.pipedev,
                process,
                &mut pipedev,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if res == FALSE {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            return Err(io::Error::last_os_error());
        }
        let pipeevent = unsafe { CreateEventW(ptr::null_mut(), FALSE, FALSE, ptr::null_mut()) };
        if pipeevent == NULL {
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(comevent) };
            _ = unsafe { CloseHandle(pipedev) };
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            comdev,
            comevent,
            pipedev,
            pipeevent,
        })
    }

    fn read(&mut self, which: WhichHandle, buf: &mut [u8]) -> io::Result<usize> {
        let (handle, event) = match which {
            WhichHandle::Pipe => (self.pipedev, self.pipeevent),
            WhichHandle::Serial => (self.comdev, self.comevent),
        };
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = event;
        let res: BOOL = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as DWORD,
                ptr::null_mut(),
                &mut overlapped,
            )
        };
        // async read request may succeed immediately, queue successfully, or fail.
        // even if it returns TRUE, the number of bytes read should be retrieved via
        // GetOverlappedResult().
        if res == FALSE && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        let mut len: DWORD = 0;
        let res: BOOL =
            unsafe { GetOverlappedResult(self.comdev, &mut overlapped, &mut len, TRUE) };
        if res == FALSE {
            return Err(io::Error::last_os_error());
        }
        match len {
            0 if buf.len() == 0 => Ok(0),
            0 => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "ReadFile() timed out (0 bytes read)",
            )),
            _ => Ok(len as usize),
        }
    }

    fn write(&mut self, which: WhichHandle, buf: &[u8]) -> io::Result<usize> {
        let (handle, event) = match which {
            WhichHandle::Pipe => (self.pipedev, self.pipeevent),
            WhichHandle::Serial => (self.comdev, self.comevent),
        };
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = event;
        let res: BOOL = unsafe {
            WriteFile(
                handle,
                buf.as_ptr() as *const c_void,
                buf.len() as DWORD,
                ptr::null_mut(),
                &mut overlapped,
            )
        };
        // async write request may succeed immediately, queue successfully, or fail.
        // even if it returns TRUE, the number of bytes written should be retrieved
        // via GetOverlappedResult().
        if res == FALSE && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        let mut len: DWORD = 0;
        let res: BOOL =
            unsafe { GetOverlappedResult(self.comdev, &mut overlapped, &mut len, TRUE) };
        if res == FALSE {
            return Err(io::Error::last_os_error());
        }
        match len {
            0 if buf.len() == 0 => Ok(0),
            0 => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "WriteFile() timed out (0 bytes written)",
            )),
            _ => Ok(len as usize),
        }
    }
}

impl Drop for Pipe2Serial {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.comdev) };
        let _ = unsafe { CloseHandle(self.pipedev) };
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        println!("Usage: {} COMn pipe-name", args[0]);
        return;
    }

    let mut p2s = Pipe2Serial::open(&args[1], &args[2]).expect("Opening COM/pipe failed");
    let mut p2s_clone = p2s.try_clone().expect("Cloning COM/pipe failed");

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let res = p2s.read(WhichHandle::Serial, &mut buf);
            match res {
                Ok(res) => {
                    if let Ok(buf_str) = std::str::from_utf8(&buf[0..res]) {
                        print!("{}", buf_str);
                        io::stdout().flush().ok();
                    }
                    let mut written = 0;
                    loop {
                        if written == res {
                            break;
                        }
                        match p2s.write(WhichHandle::Pipe, &buf[written..res]) {
                            Ok(wrote) => {
                                written += wrote;
                            }
                            Err(err) => println!("Error writing to pipe {err:}"),
                        }
                    }
                }
                Err(err) => println!("Error reading from serial {err:}"),
            }
        }
    });

    let mut buf = [0u8; 4096];
    loop {
        match p2s_clone.read(WhichHandle::Pipe, &mut buf) {
            Ok(res) => {
                if let Ok(buf_str) = std::str::from_utf8(&buf[0..res]) {
                    print!("{}", buf_str);
                    io::stdout().flush().ok();
                }
                let mut written = 0;
                loop {
                    if written == res {
                        break;
                    }
                    match p2s_clone.write(WhichHandle::Serial, &buf[written..res]) {
                        Ok(wrote) => {
                            written += wrote;
                        }
                        Err(err) => println!("Error writing to pipe {err:}"),
                    }
                }
            }
            Err(err) => println!("Error reading from pipe {err:}"),
        }
    }
}
