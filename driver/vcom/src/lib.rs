// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The RustModem virtual COM port: a UMDF 2 driver.
//!
//! Each port is one device. A host opens it by its COM name and has a
//! serial port; the RustModem service opens the same device with
//! `\control` after the name and is the modem's side of it. Everything a
//! port does is decided by a [`vcomcore::Port`], which knows nothing of
//! Windows. This crate is the layer between: it makes the device, gives
//! each request to the port, and completes requests as the port says.
//!
//! It is `unsafe` nearly throughout, because every call into the
//! framework is. What keeps it honest is that it decides nothing: there
//! is no rule of a serial port in this crate, only the plumbing.
//!
//! **How a request lives.** A request the port can answer at once is
//! completed at once. One that must wait is held by the port under the
//! value of its handle, and is marked cancelable. The port's lock is held
//! while a request is marked, and while it is unmarked before being
//! completed, so a request is completed exactly once: by whoever takes it
//! out of the port, or, if the framework has already begun to cancel it,
//! by the cancel routine.
//!
//! **Time.** The port is given milliseconds since the device started. A
//! thread for each device sleeps until the port's next deadline, or until
//! something else has been done to the port, and then tells it the time.
//! A user-mode driver has `std`, and that thread is a `std` thread.

mod win;

use core::ffi::c_void;
use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vcomcore::ioctl::{Finished, Outcome, control_ioctl, fault_status, host_ioctl, status};
use vcomcore::{Port, RequestId};
// Every use of the macro is in an `unsafe` block all the same: it is the
// macro's own rule. The short name keeps the calls readable.
#[allow(clippy::unsafe_removed_from_name)]
use wdk_sys::call_unsafe_wdf_function_binding as wdf;
use wdk_sys::{
    GUID, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, PVOID, PWDFDEVICE_INIT, STATUS_CANCELLED,
    STATUS_SUCCESS, ULONG, UNICODE_STRING, WDF_DRIVER_CONFIG, WDF_FILEOBJECT_CONFIG,
    WDF_IO_QUEUE_CONFIG, WDF_NO_OBJECT_ATTRIBUTES, WDF_OBJECT_ATTRIBUTES, WDFDEVICE, WDFDRIVER,
    WDFFILEOBJECT, WDFKEY, WDFOBJECT, WDFQUEUE, WDFREQUEST,
};

/// `STATUS_OBJECT_NAME_NOT_FOUND`: an open with a name after the port's
/// that is not the control side's.
const NAME_NOT_FOUND: u32 = 0xC000_0034;
/// `STATUS_UNSUCCESSFUL`.
const UNSUCCESSFUL: NTSTATUS = 0xC000_0001_u32 as NTSTATUS;

/// `GUID_DEVINTERFACE_COMPORT`, by which programs find serial ports.
const COMPORT_INTERFACE: GUID = GUID {
    Data1: 0x86E0_D1E0,
    Data2: 0x8089,
    Data3: 0x11D0,
    Data4: [0x9C, 0xE4, 0x08, 0x00, 0x3E, 0x30, 0x1F, 0x73],
};

/// The interface every RustModem port has.
const RUSTMODEM_INTERFACE: GUID = GUID {
    Data1: vcomapi::INTERFACE_GUID.0,
    Data2: vcomapi::INTERFACE_GUID.1,
    Data3: vcomapi::INTERFACE_GUID.2,
    Data4: vcomapi::INTERFACE_GUID.3,
};

/// What a device's lock guards.
struct Inner {
    port: Port,
    /// The requests the port holds that are marked cancelable.
    marked: HashSet<u64>,
    /// The file object of the host's open, and of the service's.
    host_file: Option<usize>,
    control_file: Option<usize>,
    /// Counts every turn that may have changed the port's deadline.
    generation: u64,
    stopping: bool,
}

/// One port.
struct Device {
    handle: usize,
    port_name: String,
    inner: Mutex<Inner>,
    changed: Condvar,
    started: Instant,
    /// The `SERIALCOMM` key and the name of this port's value in it.
    devicemap: Option<(usize, Vec<u16>)>,
    timer: Mutex<Option<JoinHandle<()>>>,
}

// SAFETY: the raw handles a Device holds are the framework's, which may be
// used from any thread; everything else is behind a lock.
unsafe impl Send for Device {}
// SAFETY: as above.
unsafe impl Sync for Device {}

impl Device {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

/// Every port of this host process.
static DEVICES: Mutex<Vec<Arc<Device>>> = Mutex::new(Vec::new());

fn devices() -> MutexGuard<'static, Vec<Arc<Device>>> {
    DEVICES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn find(device: WDFDEVICE) -> Option<Arc<Device>> {
    devices()
        .iter()
        .find(|d| d.handle == device as usize)
        .cloned()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A counted string over a buffer that ends in a NUL. The buffer must
/// outlive the string's use.
fn unicode(buffer: &[u16]) -> UNICODE_STRING {
    UNICODE_STRING {
        Length: ((buffer.len() - 1) * 2) as u16,
        MaximumLength: (buffer.len() * 2) as u16,
        Buffer: buffer.as_ptr().cast_mut(),
    }
}

fn size_of_as_ulong<T>() -> ULONG {
    size_of::<T>() as ULONG
}

fn request_of(id: RequestId) -> WDFREQUEST {
    id.0 as usize as WDFREQUEST
}

fn id_of(request: WDFREQUEST) -> RequestId {
    RequestId(request as usize as u64)
}

// ----- Turns -----

fn drain(port: &mut Port) -> Vec<Finished> {
    std::iter::from_fn(|| port.take_completion())
        .map(|completion| completion.finish())
        .collect()
}

/// Does something to a port, then completes every request that ended by
/// it. `skip` is a request the caller completes itself.
fn turn<R>(
    device: &Device,
    skip: Option<RequestId>,
    bump: bool,
    act: impl FnOnce(&mut Inner, u64, &mut Vec<Finished>) -> R,
) -> R {
    let mut ready = Vec::new();
    let result;
    {
        let mut inner = device.lock();
        let now = device.now();
        let mut ended = Vec::new();
        result = act(&mut inner, now, &mut ended);
        ended.extend(drain(&mut inner.port));
        for finished in ended {
            if Some(finished.id) == skip {
                continue;
            }
            if inner.marked.remove(&finished.id.0) {
                // SAFETY: the request was marked by `submit` and has not
                // been completed: only what leaves `marked` under this
                // lock is completed.
                let unmarked = unsafe { wdf!(WdfRequestUnmarkCancelable, request_of(finished.id)) };
                if unmarked == STATUS_CANCELLED {
                    // The framework is cancelling it: the cancel routine
                    // completes it.
                    continue;
                }
            }
            ready.push(finished);
        }
        if bump {
            inner.generation = inner.generation.wrapping_add(1);
        }
    }
    if bump {
        device.changed.notify_all();
    }
    for finished in ready {
        complete(finished);
    }
    result
}

/// Completes a request the driver owns outright.
fn complete(finished: Finished) {
    let request = request_of(finished.id);
    let mut code = finished.status as NTSTATUS;
    let mut information = finished.information;
    // SAFETY: the request is owned by the caller and not yet completed.
    // Its output buffer is written within the length the framework gives
    // for it, which the retrieve call checks is enough.
    unsafe {
        if !finished.data.is_empty() {
            let mut buffer: PVOID = core::ptr::null_mut();
            let mut length: usize = 0;
            let got = wdf!(
                WdfRequestRetrieveOutputBuffer,
                request,
                finished.data.len(),
                &mut buffer,
                &mut length,
            );
            if got >= 0 && !buffer.is_null() && length >= finished.data.len() {
                core::ptr::copy_nonoverlapping(
                    finished.data.as_ptr(),
                    buffer.cast::<u8>(),
                    finished.data.len(),
                );
            } else {
                code = if got < 0 { got } else { UNSUCCESSFUL };
                information = 0;
            }
        }
        wdf!(
            WdfRequestCompleteWithInformation,
            request,
            code,
            information as u64,
        );
    }
}

fn complete_with(request: WDFREQUEST, code: u32) {
    complete(Finished {
        id: id_of(request),
        status: code,
        data: Vec::new(),
        information: 0,
    });
}

/// Gives a request to the port and does as the port says with it.
fn submit(
    device: &Device,
    request: WDFREQUEST,
    op: impl FnOnce(&mut Inner, RequestId, u64) -> Outcome,
) {
    let id = id_of(request);
    let outcome = turn(device, None, true, |inner, now, ended| {
        let outcome = op(inner, id, now);
        if outcome == Outcome::Pending {
            ended.extend(drain(&mut inner.port));
            if !ended.iter().any(|finished| finished.id == id) {
                // The port has it. From here it can be cancelled.
                // SAFETY: the request is ours, not completed, and the
                // lock is held, so nobody can take it out of the port
                // before it is marked.
                let marked = unsafe { wdf!(WdfRequestMarkCancelableEx, request, Some(on_cancel)) };
                if marked >= 0 {
                    inner.marked.insert(id.0);
                } else {
                    // Cancelled before it could be marked.
                    inner.port.cancel(id, now);
                    ended.extend(drain(&mut inner.port));
                    ended.retain(|finished| finished.id != id);
                    ended.push(Finished {
                        id,
                        status: marked as u32,
                        data: Vec::new(),
                        information: 0,
                    });
                }
            }
        }
        outcome
    });
    if let Outcome::Done { status, data } = outcome {
        complete(Finished {
            id,
            status,
            information: data.len(),
            data,
        });
    }
}

/// The framework cancels a request the port holds.
extern "C" fn on_cancel(request: WDFREQUEST) {
    let id = id_of(request);
    let all: Vec<Arc<Device>> = devices().clone();
    for device in all {
        let ours = turn(&device, Some(id), true, |inner, now, _| {
            if inner.marked.remove(&id.0) {
                inner.port.cancel(id, now);
                true
            } else {
                false
            }
        });
        if ours {
            break;
        }
    }
    // Whether the port still had it or had just let it go, it is this
    // routine's to complete.
    complete_with(request, status::CANCELLED);
}

/// The thread that tells a port the time.
fn keep_time(device: Arc<Device>) {
    loop {
        let next = turn(&device, None, false, |inner, now, _| {
            if inner.stopping {
                return None;
            }
            inner.port.advance(now);
            let delay = inner.port.next_deadline().map(|at| at.saturating_sub(now));
            Some((delay, inner.generation))
        });
        let Some((delay, generation)) = next else {
            return;
        };
        let guard = device.lock();
        let unchanged = |inner: &mut Inner| inner.generation == generation && !inner.stopping;
        match delay {
            Some(milliseconds) => {
                let wait = Duration::from_millis(milliseconds.max(1));
                drop(
                    device
                        .changed
                        .wait_timeout_while(guard, wait, unchanged)
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
            }
            None => drop(
                device
                    .changed
                    .wait_while(guard, unchanged)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ),
        }
    }
}

// ----- The driver and its devices -----

/// Where the framework's stub calls the driver when it is loaded.
///
/// # Safety
///
/// For the framework to call, with the driver object and registry path it
/// was itself given.
// SAFETY: the name the framework's stub looks for. Nothing else has it.
#[unsafe(export_name = "DriverEntry")]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    wdk_panic::install_panic_hook();
    win::trace_start();
    let mut config = WDF_DRIVER_CONFIG {
        Size: size_of_as_ulong::<WDF_DRIVER_CONFIG>(),
        EvtDriverDeviceAdd: Some(device_add),
        ..Default::default()
    };
    // SAFETY: the framework's own arguments, and a config that lives for
    // the call.
    unsafe {
        wdf!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut config,
            core::ptr::null_mut::<WDFDRIVER>(),
        )
    }
}

/// Reads what the device's hardware key has for the port: its name, which
/// the Ports class installer or setup wrote, and how much to trace.
///
/// SAFETY: `device` is a device of this driver.
unsafe fn hardware_settings(device: WDFDEVICE) -> (Option<String>, u32) {
    // SAFETY: the buffers are locals, given with their sizes.
    unsafe {
        let mut key: WDFKEY = core::ptr::null_mut();
        let opened = wdf!(
            WdfDeviceOpenRegistryKey,
            device,
            1,           // PLUGPLAY_REGKEY_DEVICE
            0x0002_0019, // KEY_READ
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut key,
        );
        if opened < 0 {
            return (None, 0);
        }
        let name = wide("PortName");
        let name = unicode(&name);
        let mut buffer = [0u16; 64];
        let mut length: ULONG = 0;
        let mut kind: ULONG = 0;
        let read = wdf!(
            WdfRegistryQueryValue,
            key,
            &name,
            (buffer.len() * 2) as ULONG,
            buffer.as_mut_ptr().cast(),
            &mut length,
            &mut kind,
        );
        let port = (read >= 0)
            .then(|| {
                let text = &buffer[..(length as usize / 2).min(buffer.len())];
                String::from_utf16_lossy(text)
                    .trim_end_matches('\0')
                    .to_string()
            })
            .filter(|name| !name.is_empty());
        let level_name = wide("TraceLevel");
        let level_name = unicode(&level_name);
        let mut level: ULONG = 0;
        if wdf!(WdfRegistryQueryULong, key, &level_name, &mut level) < 0 {
            level = 0;
        }
        wdf!(WdfRegistryClose, key);
        (port, level)
    }
}

extern "C" fn device_add(_driver: WDFDRIVER, mut device_init: PWDFDEVICE_INIT) -> NTSTATUS {
    // SAFETY: throughout, handles are the framework's or made here, and
    // every pointer is to a local that outlives the call it is given to.
    unsafe {
        let mut files = WDF_FILEOBJECT_CONFIG {
            Size: size_of_as_ulong::<WDF_FILEOBJECT_CONFIG>(),
            EvtDeviceFileCreate: Some(file_create),
            EvtFileCleanup: Some(file_cleanup),
            AutoForwardCleanupClose: wdk_sys::_WDF_TRI_STATE::WdfUseDefault,
            FileObjectClass: wdk_sys::_WDF_FILEOBJECT_CLASS::WdfFileObjectWdfCannotUseFsContexts,
            ..Default::default()
        };
        wdf!(
            WdfDeviceInitSetFileObjectConfig,
            device_init,
            &mut files,
            WDF_NO_OBJECT_ATTRIBUTES,
        );

        let mut attributes = WDF_OBJECT_ATTRIBUTES {
            Size: size_of_as_ulong::<WDF_OBJECT_ATTRIBUTES>(),
            EvtCleanupCallback: Some(device_cleanup),
            ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
            SynchronizationScope:
                wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
            ..Default::default()
        };
        let mut device: WDFDEVICE = core::ptr::null_mut();
        let created = wdf!(
            WdfDeviceCreate,
            &mut device_init,
            &mut attributes,
            &mut device,
        );
        if created < 0 {
            return created;
        }

        let (port_name, level) = hardware_settings(device);
        win::trace_level(level);
        let Some(port_name) = port_name else {
            // A port with no name cannot be opened by anyone. Setup gives
            // every port one before the driver is installed on it.
            win::trace(win::ERROR, "a device has no PortName: not started");
            return UNSUCCESSFUL;
        };

        // The name programs open, and the entry they find the port by.
        let link = wide(&format!("\\DosDevices\\Global\\{port_name}"));
        let link = unicode(&link);
        let linked = wdf!(WdfDeviceCreateSymbolicLink, device, &link);
        if linked < 0 {
            win::trace(win::ERROR, &format!("{port_name}: no link: {linked:#x}"));
            return linked;
        }
        let map = wide("SERIALCOMM");
        let map = unicode(&map);
        let mut key: WDFKEY = core::ptr::null_mut();
        // KEY_SET_VALUE and no more: the framework refuses a wider access
        // to this key.
        let mapped = wdf!(
            WdfDeviceOpenDevicemapKey,
            device,
            &map,
            0x0002,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut key,
        );
        let mut devicemap = None;
        if mapped >= 0 {
            let value_name = wide(&format!("\\Device\\RustModem{port_name}"));
            let value = wide(&port_name);
            let written = wdf!(
                WdfRegistryAssignUnicodeString,
                key,
                &unicode(&value_name),
                &unicode(&value),
            );
            if written >= 0 {
                devicemap = Some((key as usize, value_name));
            }
        }
        if devicemap.is_none() {
            // The port works; programs that list ports will not list it.
            win::trace(
                win::ERROR,
                &format!("{port_name}: no SERIALCOMM entry: {mapped:#x}"),
            );
        }

        for interface in [&COMPORT_INTERFACE, &RUSTMODEM_INTERFACE] {
            let made = wdf!(
                WdfDeviceCreateDeviceInterface,
                device,
                interface,
                core::ptr::null(),
            );
            if made < 0 {
                return made;
            }
        }

        let mut queue_config = WDF_IO_QUEUE_CONFIG {
            Size: size_of_as_ulong::<WDF_IO_QUEUE_CONFIG>(),
            PowerManaged: wdk_sys::_WDF_TRI_STATE::WdfUseDefault,
            DefaultQueue: 1,
            DispatchType: wdk_sys::_WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchParallel,
            EvtIoRead: Some(io_read),
            EvtIoWrite: Some(io_write),
            EvtIoDeviceControl: Some(io_device_control),
            ..Default::default()
        };
        queue_config.Settings.Parallel.NumberOfPresentedRequests = ULONG::MAX;
        let mut queue: WDFQUEUE = core::ptr::null_mut();
        let queued = wdf!(
            WdfIoQueueCreate,
            device,
            &mut queue_config,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut queue,
        );
        if queued < 0 {
            return queued;
        }

        let state = Arc::new(Device {
            handle: device as usize,
            port_name: port_name.clone(),
            inner: Mutex::new(Inner {
                port: Port::new(),
                marked: HashSet::new(),
                host_file: None,
                control_file: None,
                generation: 0,
                stopping: false,
            }),
            changed: Condvar::new(),
            started: Instant::now(),
            devicemap,
            timer: Mutex::new(None),
        });
        let for_timer = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name(format!("rmvcom-{port_name}"))
            .spawn(move || keep_time(for_timer));
        match spawned {
            Ok(thread) => {
                *state.timer.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread);
            }
            Err(_) => return UNSUCCESSFUL,
        }
        devices().push(state);
        win::trace(win::INFO, &format!("{port_name}: started"));
    }
    STATUS_SUCCESS
}

/// The device is going: stop its thread, take its entry out.
extern "C" fn device_cleanup(object: WDFOBJECT) {
    let handle = object as usize;
    let device = {
        let mut all = devices();
        let at = all.iter().position(|d| d.handle == handle);
        at.map(|at| all.remove(at))
    };
    let Some(device) = device else { return };
    device.lock().stopping = true;
    device.changed.notify_all();
    let thread = device
        .timer
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    if let Some(thread) = thread {
        let _ = thread.join();
    }
    if let Some((key, value_name)) = &device.devicemap {
        // SAFETY: the key was opened for the device and is closed with
        // it, which is after this callback returns.
        unsafe {
            let _ = wdf!(WdfRegistryRemoveValue, *key as WDFKEY, &unicode(value_name));
        }
    }
    win::trace(win::INFO, &format!("{}: removed", device.port_name));
}

// ----- Opening and closing -----

/// Runs as the caller of a create that asks for the control side.
extern "C" fn check_caller(_request: WDFREQUEST, context: PVOID) {
    let allowed = win::caller_may_control();
    // SAFETY: the context is the `bool` of `file_create`, which waits for
    // this callback to return.
    unsafe {
        *context.cast::<bool>() = allowed;
    }
}

extern "C" fn file_create(device: WDFDEVICE, request: WDFREQUEST, file: WDFFILEOBJECT) {
    let Some(state) = find(device) else {
        complete_with(request, status::INVALID_DEVICE_STATE);
        return;
    };
    // SAFETY: the file object is the framework's, for this call; its name
    // is read within the length it is counted by.
    let name = unsafe {
        let name = wdf!(WdfFileObjectGetFileName, file);
        if name.is_null() || (*name).Buffer.is_null() {
            String::new()
        } else {
            let text = core::slice::from_raw_parts((*name).Buffer, (*name).Length as usize / 2);
            String::from_utf16_lossy(text)
        }
    };
    let file = file as usize;
    let code = if name.is_empty() || name == "\\" {
        // The host.
        turn(&state, None, true, |inner, now, _| {
            match inner.port.host_open(now) {
                Ok(()) => {
                    inner.host_file = Some(file);
                    status::SUCCESS
                }
                Err(fault) => fault_status(fault),
            }
        })
    } else if name.eq_ignore_ascii_case(vcomapi::CONTROL_NAME) {
        // The service. Who is asking?
        let mut allowed = false;
        // SAFETY: the callback runs before this call returns, and writes
        // the bool it is given.
        let impersonated = unsafe {
            wdf!(
                WdfRequestImpersonate,
                request,
                wdk_sys::_SECURITY_IMPERSONATION_LEVEL::SecurityIdentification,
                Some(check_caller),
                (&raw mut allowed).cast::<c_void>(),
            )
        };
        if impersonated < 0 || !allowed {
            status::ACCESS_DENIED
        } else {
            turn(&state, None, true, |inner, now, _| {
                match inner.port.control_open(now) {
                    Ok(()) => {
                        inner.control_file = Some(file);
                        status::SUCCESS
                    }
                    Err(fault) => fault_status(fault),
                }
            })
        }
    } else {
        NAME_NOT_FOUND
    };
    if win::tracing(win::INFO) {
        let port = &state.port_name;
        win::trace(win::INFO, &format!("{port}: open {name:?}: {code:#x}"));
    }
    complete_with(request, code);
}

/// The last handle to an open has gone.
extern "C" fn file_cleanup(file: WDFFILEOBJECT) {
    // SAFETY: the file object is the framework's, for this call.
    let device = unsafe { wdf!(WdfFileObjectGetDevice, file) };
    let Some(state) = find(device) else { return };
    let file = file as usize;
    let side = turn(&state, None, true, |inner, now, _| {
        if inner.control_file == Some(file) {
            inner.control_file = None;
            inner.port.control_close(now);
            "control"
        } else if inner.host_file == Some(file) {
            inner.host_file = None;
            inner.port.host_close(now);
            "host"
        } else {
            // An open that was refused.
            ""
        }
    });
    if !side.is_empty() && win::tracing(win::INFO) {
        win::trace(win::INFO, &format!("{}: {side} closed", state.port_name));
    }
}

// ----- Requests -----

/// The port a request is for, and whether it came through the service's
/// open.
///
/// SAFETY: `queue` and `request` are the framework's, for this call.
unsafe fn route(queue: WDFQUEUE, request: WDFREQUEST) -> Option<(Arc<Device>, usize)> {
    // SAFETY: as the caller promises.
    unsafe {
        let device = wdf!(WdfIoQueueGetDevice, queue);
        let file = wdf!(WdfRequestGetFileObject, request) as usize;
        find(device).map(|state| (state, file))
    }
}

extern "C" fn io_read(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    // SAFETY: the framework's handles, for this call.
    let Some((state, file)) = (unsafe { route(queue, request) }) else {
        complete_with(request, status::INVALID_DEVICE_STATE);
        return;
    };
    submit(&state, request, |inner, id, now| {
        if inner.control_file == Some(file) {
            // The service reads with its own code.
            return Outcome::Done {
                status: status::INVALID_PARAMETER,
                data: Vec::new(),
            };
        }
        match inner.port.host_read(id, length, now) {
            Ok(()) => Outcome::Pending,
            Err(fault) => Outcome::Done {
                status: fault_status(fault),
                data: Vec::new(),
            },
        }
    });
}

/// The input buffer of a request, copied out.
///
/// SAFETY: `request` is the framework's, not completed.
unsafe fn input_of(request: WDFREQUEST, length: usize) -> Result<Vec<u8>, NTSTATUS> {
    if length == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: the buffer is read within the length the framework gives
    // for it.
    unsafe {
        let mut buffer: PVOID = core::ptr::null_mut();
        let mut size: usize = 0;
        let got = wdf!(
            WdfRequestRetrieveInputBuffer,
            request,
            0,
            &mut buffer,
            &mut size,
        );
        if got < 0 {
            return Err(got);
        }
        if buffer.is_null() {
            return Ok(Vec::new());
        }
        Ok(core::slice::from_raw_parts(buffer.cast::<u8>(), size.min(length)).to_vec())
    }
}

extern "C" fn io_write(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    // SAFETY: the framework's handles, for this call.
    let Some((state, file)) = (unsafe { route(queue, request) }) else {
        complete_with(request, status::INVALID_DEVICE_STATE);
        return;
    };
    // SAFETY: the request is ours until completed.
    let data = match unsafe { input_of(request, length) } {
        Ok(data) => data,
        Err(code) => {
            complete_with(request, code as u32);
            return;
        }
    };
    submit(&state, request, |inner, id, now| {
        if inner.control_file == Some(file) {
            return Outcome::Done {
                status: status::INVALID_PARAMETER,
                data: Vec::new(),
            };
        }
        match inner.port.host_write(id, &data, now) {
            Ok(()) => Outcome::Pending,
            Err(fault) => Outcome::Done {
                status: fault_status(fault),
                data: Vec::new(),
            },
        }
    });
}

extern "C" fn io_device_control(
    queue: WDFQUEUE,
    request: WDFREQUEST,
    output_length: usize,
    input_length: usize,
    code: ULONG,
) {
    // SAFETY: the framework's handles, for this call.
    let Some((state, file)) = (unsafe { route(queue, request) }) else {
        complete_with(request, status::INVALID_DEVICE_STATE);
        return;
    };
    // SAFETY: the request is ours until completed.
    let input = match unsafe { input_of(request, input_length) } {
        Ok(input) => input,
        Err(failed) => {
            complete_with(request, failed as u32);
            return;
        }
    };
    if win::tracing(win::VERBOSE) {
        let port = &state.port_name;
        win::trace(
            win::VERBOSE,
            &format!("{port}: control code {code:#x}, in {input_length}, out {output_length}"),
        );
    }
    submit(&state, request, |inner, id, now| {
        if inner.control_file == Some(file) {
            control_ioctl(&mut inner.port, id, code, &input, output_length, now)
        } else {
            host_ioctl(&mut inner.port, id, code, &input, output_length, now)
        }
    });
}
