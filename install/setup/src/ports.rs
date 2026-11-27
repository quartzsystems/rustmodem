// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The ports of the lines: `port add`, `port remove` and `port list`.
//!
//! A RustModem port is two devices, a COM port and the modem attached to
//! it, and to a person it is one thing with a COM name. These commands
//! make both, remove both, and list them as one. They work through a
//! [`Devices`], which on Windows is SetupAPI and in the tests is a table.

use msg::{Message, message};

/// The first COM number setup gives a port it is not told the number
/// of: clear of the numbers real ports and USB adapters take.
pub const FIRST_AUTOMATIC: u32 = 10;
/// The greatest COM number there is.
pub const HIGHEST: u32 = 256;

/// A device, as the device layer lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub instance: String,
    pub name: String,
    /// The COM name a port has or a modem is attached to.
    pub com: Option<String>,
    pub driver_version: Option<String>,
    pub started: bool,
}

/// The device layer.
pub trait Devices {
    /// Whether the driver package is in the driver store.
    fn driver_installed(&mut self) -> bool;
    /// The RustModem port devices.
    fn ports(&mut self) -> Result<Vec<Device>, String>;
    /// The RustModem modem devices.
    fn modems(&mut self) -> Result<Vec<Device>, String>;
    /// The COM numbers Windows has as taken.
    fn taken(&mut self) -> Result<Vec<u32>, String>;
    /// Marks a COM number as taken. False if it already was.
    fn claim(&mut self, number: u32) -> Result<bool, String>;
    /// Marks a COM number as free.
    fn release(&mut self, number: u32) -> Result<(), String>;
    /// Makes a port device named `com`. Gives its instance id.
    fn create_port(&mut self, com: &str) -> Result<String, String>;
    /// Makes a modem device attached to `com`. Gives its instance id.
    fn create_modem(&mut self, com: &str) -> Result<String, String>;
    /// Removes a device.
    fn remove(&mut self, instance: &str) -> Result<(), String>;
}

/// A line's port, as a person sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    /// `COM10`.
    pub com: String,
    pub port_instance: String,
    /// The modem attached to it, if there is one, and its name.
    pub modem_instance: Option<String>,
    pub modem_name: Option<String>,
    pub driver_version: Option<String>,
    /// Both devices are started.
    pub started: bool,
}

/// The number of a COM name, `COM10` or `com10`.
pub fn com_number(name: &str) -> Option<u32> {
    let digits = name
        .get(..3)
        .filter(|prefix| prefix.eq_ignore_ascii_case("COM"))
        .and(name.get(3..))?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| (1..=HIGHEST).contains(n))
}

fn same_com(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn listing(devices: &mut dyn Devices) -> Result<Vec<Port>, String> {
    let modems = devices.modems()?;
    let mut ports: Vec<Port> = devices
        .ports()?
        .into_iter()
        .filter_map(|port| {
            let com = port.com.clone()?;
            let modem = modems
                .iter()
                .find(|modem| modem.com.as_deref().is_some_and(|c| same_com(c, &com)));
            Some(Port {
                started: port.started && modem.is_some_and(|m| m.started),
                modem_instance: modem.map(|m| m.instance.clone()),
                modem_name: modem.map(|m| m.name.clone()),
                driver_version: port.driver_version,
                port_instance: port.instance,
                com,
            })
        })
        .collect();
    ports.sort_by_key(|port| com_number(&port.com).unwrap_or(u32::MAX));
    Ok(ports)
}

/// `port list`: the RustModem ports, in the order of their numbers.
pub fn list(devices: &mut dyn Devices) -> Result<Vec<Port>, Message> {
    listing(devices).map_err(|detail| message("RMD9012", &[&detail]))
}

/// `port add`: makes `count` ports, the first at `com` if a number is
/// given and the rest at the free numbers after it. Gives their names.
/// Either all are made or none is: a port that cannot be finished is
/// taken away again, with those made before it.
pub fn add(
    devices: &mut dyn Devices,
    com: Option<u32>,
    count: u32,
) -> Result<Vec<String>, Message> {
    if !devices.driver_installed() {
        return Err(message("RMD9010", &[]));
    }
    let failed = |name: &str, detail: &str| message("RMD9008", &[name, detail]);
    let taken = devices.taken().map_err(|detail| failed("", &detail))?;
    let existing = listing(devices).map_err(|detail| failed("", &detail))?;
    let in_use = |number: u32| {
        taken.contains(&number)
            || existing
                .iter()
                .any(|port| com_number(&port.com) == Some(number))
    };
    if let Some(number) = com
        && (!(1..=HIGHEST).contains(&number) || in_use(number))
    {
        return Err(message("RMD9006", &[&format!("COM{number}")]));
    }
    let mut numbers = Vec::new();
    let mut next = com.unwrap_or(FIRST_AUTOMATIC);
    while (numbers.len() as u32) < count {
        if next > HIGHEST {
            return Err(message("RMD9011", &[]));
        }
        if !in_use(next) {
            numbers.push(next);
        }
        next += 1;
    }

    let mut made: Vec<(u32, Vec<String>)> = Vec::new();
    let mut result = Ok(());
    for number in &numbers {
        let name = format!("COM{number}");
        let mut instances = Vec::new();
        let step = (|| -> Result<(), String> {
            if !devices.claim(*number)? {
                return Err(format!("{name} was taken while the port was being made"));
            }
            instances.push(devices.create_port(&name)?);
            instances.push(devices.create_modem(&name)?);
            Ok(())
        })();
        made.push((*number, instances));
        if let Err(detail) = step {
            result = Err(failed(&name, &detail));
            break;
        }
    }
    if result.is_err() {
        // Undone last first: the modem before its port.
        for (number, instances) in made.iter().rev() {
            for instance in instances.iter().rev() {
                let _ = devices.remove(instance);
            }
            let _ = devices.release(*number);
        }
    }
    result.map(|()| numbers.iter().map(|n| format!("COM{n}")).collect())
}

/// `port remove`: removes the port named `com` and its modem.
pub fn remove(devices: &mut dyn Devices, com: &str) -> Result<(), Message> {
    let failed = |detail: &str| message("RMD9009", &[com, detail]);
    let ports = listing(devices).map_err(|detail| failed(&detail))?;
    let Some(port) = ports.iter().find(|port| same_com(&port.com, com)) else {
        return Err(message("RMD9001", &[com]));
    };
    remove_port(devices, port).map_err(|detail| failed(&detail))
}

fn remove_port(devices: &mut dyn Devices, port: &Port) -> Result<(), String> {
    // The modem first: it is attached to the port.
    if let Some(modem) = &port.modem_instance {
        devices.remove(modem)?;
    }
    devices.remove(&port.port_instance)?;
    if let Some(number) = com_number(&port.com) {
        devices.release(number)?;
    }
    Ok(())
}

/// Removes every RustModem port, and any RustModem modem left with no
/// port. Gives what could not be removed.
pub fn remove_all(devices: &mut dyn Devices) -> Result<(), String> {
    let mut failures = Vec::new();
    for port in listing(devices)? {
        if let Err(detail) = remove_port(devices, &port) {
            failures.push(format!("{}: {detail}", port.com));
        }
    }
    for modem in devices.modems()? {
        if let Err(detail) = devices.remove(&modem.instance) {
            failures.push(format!("{}: {detail}", modem.instance));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
