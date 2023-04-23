use std::ffi::OsStr;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use std::env;
use std::thread;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{BOOL, DWORD, FALSE, TRUE};
use winapi::shared::winerror::ERROR_PIPE_BUSY;
use winapi::um::commapi::{SetCommState, SetCommTimeouts};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::fileapi::{CreateFileW, FlushFileBuffers, ReadFile, WriteFile, OPEN_EXISTING};
use winapi::um::handleapi::{CloseHandle, DuplicateHandle, INVALID_HANDLE_VALUE};
use winapi::um::namedpipeapi::{SetNamedPipeHandleState, WaitNamedPipeW};
use winapi::um::processthreadsapi::GetCurrentProcess;
use winapi::um::winbase::{CBR_115200, COMMTIMEOUTS, DCB, NOPARITY, ONESTOPBIT};
use winapi::um::winbase::{PIPE_READMODE_BYTE, PIPE_WAIT};
use winapi::um::winnt::{
    DUPLICATE_SAME_ACCESS, FILE_ATTRIBUTE_NORMAL, GENERIC_READ, GENERIC_WRITE, HANDLE,
};

enum WhichHandle {
    Pipe,
    Serial,
}

pub struct Pipe2Serial {
    comdev: HANDLE,
    pipedev: HANDLE,
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
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if comdev == INVALID_HANDLE_VALUE {
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
            let error = io::Error::last_os_error();
            _ = unsafe { CloseHandle(comdev) };
            return Err(error);
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
            let error = io::Error::last_os_error();
            _ = unsafe { CloseHandle(comdev) };
            return Err(error);
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
                    0,
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
            return Err(io::Error::last_os_error());
        }
        let mut dw_mode: DWORD = PIPE_READMODE_BYTE | PIPE_WAIT;
        let res = unsafe {
            SetNamedPipeHandleState(
                pipedev,         // pipe handle
                &mut dw_mode,    // new pipe mode
                ptr::null_mut(), // don't set maximum bytes
                ptr::null_mut(),
            )
        };
        if res == FALSE {
            let error = io::Error::last_os_error();
            _ = unsafe { CloseHandle(comdev) };
            _ = unsafe { CloseHandle(pipedev) };
            return Err(error);
        }

        Ok(Self { comdev, pipedev })
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
            let error = io::Error::last_os_error();
            return Err(error);
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
            let error = io::Error::last_os_error();
            _ = unsafe { CloseHandle(comdev) };
            return Err(error);
        }

        Ok(Self { comdev, pipedev })
    }

    fn read(&mut self, which: WhichHandle, buf: &mut [u8]) -> io::Result<usize> {
        let handle = match which {
            WhichHandle::Pipe => self.pipedev,
            WhichHandle::Serial => self.comdev,
        };
        let mut bytes_read: DWORD = 0;
        let res = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as DWORD,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };
        if res == FALSE {
            return Err(io::Error::last_os_error());
        }

        Ok(bytes_read as usize)
    }

    fn write(&mut self, which: WhichHandle, buf: &[u8]) -> io::Result<usize> {
        let handle = match which {
            WhichHandle::Pipe => self.pipedev,
            WhichHandle::Serial => self.comdev,
        };
        let mut bytes_written: DWORD = 0;
        let res: BOOL = unsafe {
            WriteFile(
                handle,
                buf.as_ptr() as *const c_void,
                buf.len() as DWORD,
                &mut bytes_written,
                ptr::null_mut(),
            )
        };

        if res == FALSE {
            return Err(io::Error::last_os_error());
        }

        Ok(bytes_written as usize)
    }

    #[allow(dead_code)]
    fn flush_pipe(&mut self) -> io::Result<()> {
        let err = unsafe { FlushFileBuffers(self.pipedev) };
        if err == 0 {
            println!("FLUSH FAILED");
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn flush_serial(&mut self) -> io::Result<()> {
        let err = unsafe { FlushFileBuffers(self.comdev) };
        if err == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
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

    println!("Opening first set");
    let mut p2s = Pipe2Serial::open(&args[1], &args[2]).expect("Opening COM/pipe failed");
    println!("opening second set");
    let mut p2s_clone = p2s.try_clone().expect("Cloning COM/pipe failed");
    println!("spawning");

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let res = p2s.read(WhichHandle::Serial, &mut buf);
            println!("DONE READ {res:?}");
            match res {
                Ok(res) => {
                    print!("{}", std::str::from_utf8(&buf[0..res]).unwrap());
                    let mut written = 0;
                    loop {
                        if written == res {
                            break;
                        }
                        match p2s.write(WhichHandle::Pipe, &buf[written..res]) {
                            Ok(wrote) => {
                                written += wrote;
                                println!("wrote to pipe {wrote:}");
                                p2s.flush_pipe().ok();
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
            Ok(res) => match p2s_clone.write(WhichHandle::Serial, &buf[0..res]) {
                Ok(_) => {}
                Err(err) => println!("Error writing to serial {err:}"),
            },
            Err(err) => println!("Error reading from pipe {err:}"),
        }
        //std::thread::sleep(std::time::Duration::from_secs(100));
    }
}
