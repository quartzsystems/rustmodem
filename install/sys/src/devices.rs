// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Root-enumerated devices, made and removed through SetupAPI.
//!
//! A RustModem line is two devices (`docs/DRIVER_SPIKE.md`): a port, of
//! class Ports, whose driver is ours, and a modem, of class Modem, with
//! no driver of its own, attached to the port's COM name. Both are made
//! here with no wizard and no restart of Windows, and both stay until
//! they are removed here.
//!
//! The sequence is the one every program that adds a device with no
//! hardware uses: an empty device is made with its hardware id and
//! registered, what its class wants to find in the registry is written,
//! and the driver that is in the driver store for that hardware id is
//! installed on it. Two things were found out by doing it:
//!
//! - **A port is registered through its class installer; a modem is
//!   not.** The Modem class installer leaves no device behind when asked
//!   to register one, so a modem is registered past it.
//! - **A name written before the driver is installed is kept.** The
//!   port's `PortName`, in its hardware key, is the COM name it comes up
//!   with; the modem's `AttachedTo`, in its driver key, is the port
//!   Windows' modem stack opens.
//!
//! The driver package must be in the driver store, and its certificate
//! trusted, before a device is made: Windows otherwise asks a question on
//! the desktop and waits for the answer.

use std::io;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVNODE_STATUS_FLAGS, CM_Get_DevNode_Status, CM_PROB, CONFIGRET, CR_SUCCESS,
    DICD_GENERATE_ID, DICS_FLAG_GLOBAL, DIF_REGISTERDEVICE, DIF_REMOVE, DIGCF_PRESENT,
    DIIDFLAG_NOFINISHINSTALLUI, DIREG_DEV, DIREG_DRV, DiInstallDevice, HDEVINFO,
    SETUP_DI_REGISTRY_PROPERTY, SP_DEVINFO_DATA, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME,
    SPDRP_HARDWAREID, SetupDiCallClassInstaller, SetupDiCreateDevRegKeyW,
    SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
    SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey, SetupDiOpenDeviceInfoW,
    SetupDiRegisterDeviceInfo, SetupDiSetDeviceRegistryPropertyW,
};
use windows::Win32::System::Registry::{
    HKEY, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegSetValueExW,
};
use windows::core::{BOOL, GUID, HSTRING, PCWSTR};

use crate::{from_wide, wide_octets};

/// The class of a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Ports (COM and LPT).
    Ports,
    /// Modems.
    Modem,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Class::Ports => "Ports",
            Class::Modem => "Modem",
        }
    }

    fn guid(self) -> GUID {
        match self {
            Class::Ports => GUID::from_u128(0x4D36E978_E325_11CE_BFC1_08002BE10318),
            Class::Modem => GUID::from_u128(0x4D36E96D_E325_11CE_BFC1_08002BE10318),
        }
    }

    /// Where the class keeps the name that joins a device to a COM port,
    /// and what it is called there.
    fn name_value(self) -> (u32, &'static str) {
        match self {
            Class::Ports => (DIREG_DEV, "PortName"),
            Class::Modem => (DIREG_DRV, "AttachedTo"),
        }
    }
}

/// A device that is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Its instance id, as `ROOT\PORTS\0000`.
    pub instance: String,
    /// Its name in Device Manager.
    pub name: String,
    /// The COM name it has, if a port, or is attached to, if a modem.
    pub com: Option<String>,
    /// The version of the driver package installed on it.
    pub driver_version: Option<String>,
    /// Whether Windows has started it.
    pub started: bool,
}

/// A device information set, destroyed when dropped.
struct Set(HDEVINFO);

impl Drop for Set {
    fn drop(&mut self) {
        // SAFETY: the set was made by this module and is destroyed once.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn info_data() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    }
}

/// A registry key of a device, closed when dropped.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this module and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl Key {
    fn string(&self, name: &str) -> Option<String> {
        let mut buffer = [0u16; 512];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer is a local, given with its size in octets.
        let read = unsafe {
            RegGetValueW(
                self.0,
                PCWSTR::null(),
                &HSTRING::from(name),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        read.is_ok()
            .then(|| from_wide(&buffer))
            .filter(|text| !text.is_empty())
    }
}

/// Makes a device of `class` with `hardware_id`, gives it its COM name,
/// and installs on it the driver the driver store has for it. Gives the
/// device's instance id. A device that could not be finished is removed.
pub fn create(class: Class, hardware_id: &str, com: &str) -> io::Result<String> {
    let guid = class.guid();
    // SAFETY: throughout, every pointer is to a local that outlives the
    // call it is given to, and the set is destroyed when `set` drops.
    unsafe {
        let set = Set(SetupDiCreateDeviceInfoList(Some(&guid), None)?);
        let mut info = info_data();
        SetupDiCreateDeviceInfoW(
            set.0,
            &HSTRING::from(class.name()),
            &guid,
            PCWSTR::null(),
            None,
            DICD_GENERATE_ID,
            Some(&mut info),
        )?;
        SetupDiSetDeviceRegistryPropertyW(
            set.0,
            &mut info,
            SPDRP_HARDWAREID,
            Some(&wide_octets(hardware_id, 2)),
        )?;
        match class {
            Class::Ports => SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, Some(&info))?,
            // The Modem class installer registers nothing.
            Class::Modem => SetupDiRegisterDeviceInfo(set.0, &mut info, 0, None, None, None)?,
        }
        // From here there is a device, which is taken away again if the
        // rest fails.
        let finished = (|| -> io::Result<String> {
            let instance = instance_id(set.0, &info)?;
            let (which, name) = class.name_value();
            let key = Key(SetupDiCreateDevRegKeyW(
                set.0,
                &info,
                DICS_FLAG_GLOBAL.0,
                0,
                which,
                None,
                PCWSTR::null(),
            )?);
            RegSetValueExW(
                key.0,
                &HSTRING::from(name),
                None,
                REG_SZ,
                Some(&wide_octets(com, 1)),
            )
            .ok()?;
            drop(key);
            let mut reboot = BOOL(0);
            // The best driver the store has for this device, on this
            // device alone, with nothing shown.
            DiInstallDevice(
                None,
                set.0,
                &info,
                None,
                DIIDFLAG_NOFINISHINSTALLUI,
                Some(&mut reboot),
            )?;
            Ok(instance)
        })();
        if finished.is_err() {
            let _ = SetupDiCallClassInstaller(DIF_REMOVE, set.0, Some(&info));
        }
        finished
    }
}

/// SAFETY: `set` and `info` are a live set and one of its devices.
unsafe fn instance_id(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> io::Result<String> {
    let mut id = [0u16; 256];
    // SAFETY: the buffer is a local, given as a slice.
    unsafe {
        SetupDiGetDeviceInstanceIdW(set, info, Some(&mut id), None)?;
    }
    Ok(from_wide(&id))
}

/// Removes a device by its instance id. A device that is not there is
/// not an error: it is what was wanted.
pub fn remove(instance: &str) -> io::Result<()> {
    // SAFETY: as in `create`.
    unsafe {
        let set = Set(SetupDiCreateDeviceInfoList(None, None)?);
        let mut info = info_data();
        if SetupDiOpenDeviceInfoW(set.0, &HSTRING::from(instance), None, 0, Some(&mut info))
            .is_err()
        {
            return Ok(());
        }
        SetupDiCallClassInstaller(DIF_REMOVE, set.0, Some(&info))?;
    }
    Ok(())
}

/// SAFETY: `set` and `info` are a live set and one of its devices.
unsafe fn property(
    set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    which: SETUP_DI_REGISTRY_PROPERTY,
) -> Vec<String> {
    let mut buffer = [0u8; 2048];
    // SAFETY: the buffer is a local, given as a slice.
    let read = unsafe {
        SetupDiGetDeviceRegistryPropertyW(set, info, which, None, Some(&mut buffer), None)
    };
    if read.is_err() {
        return Vec::new();
    }
    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    // One string, or several each ended by a NUL and the last by two.
    units
        .split(|&unit| unit == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// The devices of `class` that have `hardware_id`.
pub fn list(class: Class, hardware_id: &str) -> io::Result<Vec<Device>> {
    let guid = class.guid();
    let mut found = Vec::new();
    // SAFETY: as in `create`.
    unsafe {
        let set = Set(SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT,
        )?);
        for index in 0.. {
            let mut info = info_data();
            if SetupDiEnumDeviceInfo(set.0, index, &mut info).is_err() {
                break;
            }
            let ours = property(set.0, &info, SPDRP_HARDWAREID)
                .iter()
                .any(|id| id.eq_ignore_ascii_case(hardware_id));
            if !ours {
                continue;
            }
            let Ok(instance) = instance_id(set.0, &info) else {
                continue;
            };
            let mut name = property(set.0, &info, SPDRP_FRIENDLYNAME);
            if name.is_empty() {
                name = property(set.0, &info, SPDRP_DEVICEDESC);
            }
            let (which, value) = class.name_value();
            let open = |which: u32| {
                SetupDiOpenDevRegKey(set.0, &info, DICS_FLAG_GLOBAL.0, 0, which, KEY_READ.0)
                    .ok()
                    .map(Key)
            };
            let com = open(which).and_then(|key| key.string(value));
            let driver_version = open(DIREG_DRV).and_then(|key| key.string("DriverVersion"));
            let mut status = CM_DEVNODE_STATUS_FLAGS(0);
            let mut problem = CM_PROB(0);
            let asked: CONFIGRET =
                CM_Get_DevNode_Status(&mut status, &mut problem, info.DevInst, 0);
            // DN_STARTED.
            let started = asked == CR_SUCCESS && status.0 & 0x0000_0008 != 0;
            found.push(Device {
                instance,
                name: name.into_iter().next().unwrap_or_default(),
                com,
                driver_version,
                started,
            });
        }
    }
    Ok(found)
}
