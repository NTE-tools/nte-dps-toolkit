//! Framed, bounded native Combat push transport. No v7 messages, DLL loading,
//! packet parsing or automatic retry of a mutation. Closing owns session cleanup.
use super::toolkit::{ToolkitClient, ToolkitError};
use std::{
    io, ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_DATA, FILE_WRITE_DATA, OPEN_EXISTING,
        ReadFile, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResultEx, OVERLAPPED},
        Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe},
        Threading::CreateEventW,
    },
};

pub const MAX_FRAME: usize = 262_144;
const IPC_CANCEL_DRAIN_TIMEOUT_MS: u32 = 250;
static IPC_CLIENT_QUARANTINED: AtomicBool = AtomicBool::new(false);

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.0.is_null() || self.0 == INVALID_HANDLE_VALUE {
            return;
        }
        // SAFETY: every constructor transfers one checked, owned Win32 handle
        // into this non-cloneable wrapper and never closes it elsewhere.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct OverlappedPipeOperation {
    _event: OwnedHandle,
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u8]>,
}

impl OverlappedPipeOperation {
    fn with_buffer(buffer: Box<[u8]>) -> Result<Self, String> {
        // SAFETY: this creates one unnamed manual-reset event owned by the
        // operation. No untrusted security attributes or name are supplied.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error().to_string());
        }
        let event = OwnedHandle(event);
        let mut overlapped = Box::<OVERLAPPED>::default();
        overlapped.hEvent = event.raw();
        Ok(Self {
            _event: event,
            overlapped,
            buffer,
        })
    }

    fn quarantine(self) {
        // A cancellation that did not reach a terminal state may still access
        // OVERLAPPED, its event, and the transfer buffer. Retain exactly one
        // process-lifetime allocation and fail all future calls closed rather
        // than returning while the kernel may reference freed memory.
        IPC_CLIENT_QUARANTINED.store(true, Ordering::Release);
        std::mem::forget(self);
    }
}

enum OverlappedWaitError {
    Terminal(String),
    TimedOut,
    Undrained,
}

impl OverlappedWaitError {
    fn requires_quarantine(&self) -> bool {
        matches!(self, Self::Undrained)
    }

    fn into_message(self) -> String {
        match self {
            Self::Terminal(message) => message,
            Self::TimedOut => "Capture pipe IPC operation timed out".to_owned(),
            Self::Undrained => {
                "Capture pipe IPC cancellation did not reach a terminal state".to_owned()
            }
        }
    }
}

fn remaining_timeout_ms(deadline: Instant) -> Option<u32> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    if remaining.is_zero() {
        return None;
    }
    let millis = remaining.as_millis().max(1).min(u128::from(u32::MAX));
    Some(millis as u32)
}

fn overlapped_status_is_terminal(error: &io::Error) -> bool {
    let code = error.raw_os_error().map(|value| value as u32);
    code == Some(ERROR_OPERATION_ABORTED)
        || !matches!(
            code,
            Some(ERROR_IO_PENDING) | Some(ERROR_IO_INCOMPLETE) | Some(WAIT_TIMEOUT)
        )
}

fn await_overlapped(
    pipe: HANDLE,
    operation: &mut OverlappedPipeOperation,
    deadline: Instant,
) -> Result<u32, OverlappedWaitError> {
    if let Some(timeout) = remaining_timeout_ms(deadline) {
        let mut transferred = 0_u32;
        // SAFETY: pipe, OVERLAPPED, event, and transfer buffer remain alive for
        // this wait on the exclusively owned handle.
        if unsafe {
            GetOverlappedResultEx(
                pipe,
                operation.overlapped.as_ref(),
                &mut transferred,
                timeout,
                0,
            )
        } != 0
        {
            return Ok(transferred);
        }
        let error = io::Error::last_os_error();
        if overlapped_status_is_terminal(&error) {
            return Err(OverlappedWaitError::Terminal(error.to_string()));
        }
    }

    // SAFETY: the OVERLAPPED belongs to this exact pipe operation and both stay
    // alive through the bounded terminal-drain attempt.
    unsafe {
        CancelIoEx(pipe, operation.overlapped.as_ref());
    }
    let mut transferred = 0_u32;
    // SAFETY: same lifetime proof as above; the additional deadline bounds the
    // cancellation drain independently of a stalled peer.
    if unsafe {
        GetOverlappedResultEx(
            pipe,
            operation.overlapped.as_ref(),
            &mut transferred,
            IPC_CANCEL_DRAIN_TIMEOUT_MS,
            0,
        )
    } != 0
    {
        return Err(OverlappedWaitError::TimedOut);
    }
    let drain_error = io::Error::last_os_error();
    if overlapped_status_is_terminal(&drain_error) {
        return Err(OverlappedWaitError::TimedOut);
    }

    Err(OverlappedWaitError::Undrained)
}

fn pipe_io_ready() -> bool {
    !IPC_CLIENT_QUARANTINED.load(Ordering::Acquire)
}

fn overlapped_write_exact(pipe: HANDLE, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    let mut operation = OverlappedPipeOperation::with_buffer(bytes.to_vec().into_boxed_slice())?;
    // SAFETY: pipe is opened for overlapped writes and the boxed buffer plus
    // OVERLAPPED remain stable until completion or quarantine.
    let started = unsafe {
        WriteFile(
            pipe,
            operation.buffer.as_ptr(),
            operation.buffer.len() as u32,
            ptr::null_mut(),
            operation.overlapped.as_mut(),
        )
    };
    if started == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error().map(|code| code as u32) != Some(ERROR_IO_PENDING) {
            return Err(error.to_string());
        }
    }
    // Even when WriteFile returns TRUE on an overlapped handle, only the
    // OVERLAPPED completion result is authoritative for the byte count.
    let transferred = match await_overlapped(pipe, &mut operation, deadline) {
        Ok(transferred) => transferred,
        Err(error) => {
            if error.requires_quarantine() {
                operation.quarantine();
            }
            return Err(error.into_message());
        }
    };
    if transferred != bytes.len() as u32 {
        return Err("Capture pipe IPC request was only partially written".to_owned());
    }
    Ok(())
}

fn overlapped_read_exact(
    pipe: HANDLE,
    byte_len: usize,
    deadline: Instant,
) -> Result<Box<[u8]>, String> {
    let mut operation =
        OverlappedPipeOperation::with_buffer(vec![0_u8; byte_len].into_boxed_slice())?;
    // SAFETY: pipe is opened for overlapped reads and the boxed buffer plus
    // OVERLAPPED remain stable until completion or quarantine.
    let started = unsafe {
        ReadFile(
            pipe,
            operation.buffer.as_mut_ptr(),
            operation.buffer.len() as u32,
            ptr::null_mut(),
            operation.overlapped.as_mut(),
        )
    };
    if started == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error().map(|code| code as u32) != Some(ERROR_IO_PENDING) {
            return Err(error.to_string());
        }
    }
    // ReadFile's synchronous return does not make lpNumberOfBytesRead reliable
    // for an overlapped handle; consume the same bounded completion path.
    let transferred = match await_overlapped(pipe, &mut operation, deadline) {
        Ok(transferred) => transferred,
        Err(error) => {
            if error.requires_quarantine() {
                operation.quarantine();
            }
            return Err(error.into_message());
        }
    };
    if transferred != byte_len as u32 {
        return Err("Capture pipe IPC response was only partially read".to_owned());
    }
    Ok(operation.buffer)
}

pub struct CapturePipe {
    handle: HANDLE,
}
impl Drop for CapturePipe {
    fn drop(&mut self) {
        // SAFETY: sole owned handle, every transfer completed or retained its
        // owned operation storage on cancellation failure before this close.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
impl CapturePipe {
    pub fn open(host: &ToolkitClient) -> Result<Self, ToolkitError> {
        if !pipe_io_ready() {
            return Err(ToolkitError::Unavailable);
        }
        let (pid, created) = host.process_identity();
        let name: Vec<u16> = format!("\\\\.\\pipe\\nte.capture.v1.{pid}.{created}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: terminated name; the handle is exclusively owned, and all I/O uses
        // stable owned OVERLAPPED buffers with cancellation/drain in shared helpers.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_DATA | FILE_WRITE_DATA,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(ToolkitError::Unavailable);
        }
        let pipe = Self { handle };
        let mut server = 0;
        // SAFETY: live pipe handle and valid DWORD output; bind to the inspected host.
        if unsafe { GetNamedPipeServerProcessId(handle, &mut server) } == 0 || server != pid {
            return Err(ToolkitError::InvalidProtocol);
        }
        Ok(pipe)
    }
    pub fn send(
        &self,
        id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), ToolkitError> {
        if !pipe_io_ready() {
            return Err(ToolkitError::Unavailable);
        }
        let bytes = serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .map_err(|_| ToolkitError::InvalidProtocol)?;
        if bytes.len() > 16384 {
            return Err(ToolkitError::TooLarge);
        }
        let mut frame = (bytes.len() as u32).to_le_bytes().to_vec();
        frame.extend(bytes);
        overlapped_write_exact(self.handle, &frame, Instant::now() + Duration::from_secs(2))
            .map_err(|_| ToolkitError::Failed)
    }
    fn available(&self) -> Result<usize, ToolkitError> {
        let mut n = 0;
        // SAFETY: live sole-reader pipe, valid DWORD output; no payload access here.
        if unsafe {
            PeekNamedPipe(
                self.handle,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut n,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(ToolkitError::Unavailable);
        }
        Ok(n as usize)
    }
    fn exact(
        &self,
        n: usize,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> Result<Vec<u8>, ToolkitError> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            if stop.load(Ordering::Acquire) {
                return Err(ToolkitError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ToolkitError::Timeout);
            }
            let count = self.available()?.min(n - out.len());
            if count == 0 {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let bytes = overlapped_read_exact(self.handle, count, deadline)
                .map_err(|_| ToolkitError::Failed)?;
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }
    pub fn read(&self, stop: &AtomicBool) -> Result<Option<Vec<u8>>, ToolkitError> {
        if !pipe_io_ready() {
            return Err(ToolkitError::Unavailable);
        }
        if stop.load(Ordering::Acquire) {
            return Err(ToolkitError::Cancelled);
        }
        if self.available()? == 0 {
            return Ok(None);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let header = self.exact(4, deadline, stop)?;
        let size = u32::from_le_bytes(
            header
                .try_into()
                .map_err(|_| ToolkitError::InvalidProtocol)?,
        ) as usize;
        if size == 0 || size > MAX_FRAME {
            return Err(ToolkitError::TooLarge);
        }
        self.exact(size, deadline, stop).map(Some)
    }
}
