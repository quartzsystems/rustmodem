// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The serial ports of the machine: what `ports.list` reports and what the
//! lines' bindings are made from.
//!
//! Four things are read from the registry and put together:
//!
//! - the serial ports Windows has, from the `SERIALCOMM` device map;
//! - which of those are RustModem ports: the driver names its entry in
//!   the device map `\Device\RustModem` and the port's name;
//! - the com0com pairs, from the parameters the com0com driver keeps for
//!   each of its ports;
//! - the modems, from the Modem device class, each with the port it is
//!   attached to.
//!
//! Putting them together ([`inventory`]) is pure and tested. Reading them
//! ([`SystemPorts`]) is not tested: it needs a machine with com0com pairs
//! and a modem installed.
//! NEEDS-CODY: verify on a machine with com0com pairs and a standard modem
//! installed. The key and value names below are from the com0com ReadMe
//! and from memory of the registry of such a machine, not from one.

use ctlproto::{PortInfo, PortKind};

/// One port of a com0com pair, as the driver's parameters describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Com0comPort {
    /// The name the driver gives the port: `CNCA0`, `CNCB0` and so on. The
    /// two ports of pair `n` are `CNCA<n>` and `CNCB<n>`.
    pub driver_name: String,
    /// The `PortName` parameter: a name, or `COM#` for a name chosen by
    /// the Ports class installer, or `-` or nothing for the driver's name.
    pub port_name: String,
    /// The name the Ports class installer chose, when `PortName` is
    /// `COM#`.
    pub real_name: Option<String>,
}

impl Com0comPort {
    /// The name the port is opened by.
    pub fn name(&self) -> &str {
        match self.port_name.as_str() {
            "" | "-" => &self.driver_name,
            "COM#" => self.real_name.as_deref().unwrap_or(&self.driver_name),
            name => name,
        }
    }

    /// The driver's name for the other port of the pair.
    fn pair(&self) -> Option<String> {
        let upper = self.driver_name.to_ascii_uppercase();
        let (other, number) = if let Some(number) = upper.strip_prefix("CNCA") {
            ("CNCB", number)
        } else {
            ("CNCA", upper.strip_prefix("CNCB")?)
        };
        Some(format!("{other}{number}"))
    }
}

/// A modem device and the port it is attached to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModemDevice {
    /// The friendly name, for example `Standard 56000 bps Modem`.
    pub name: String,
    /// The port, for example `COM4`.
    pub attached_to: String,
    /// The version of the driver package the modem was installed from.
    pub driver_version: Option<String>,
}

/// What the registry says, before it is put together.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Raw {
    /// The port names in the `SERIALCOMM` device map.
    pub serialcomm: Vec<String>,
    /// Those of them that are RustModem ports.
    pub vcom: Vec<String>,
    /// The ports of the com0com pairs.
    pub com0com: Vec<Com0comPort>,
    /// The modems.
    pub modems: Vec<ModemDevice>,
}

/// The ports of the machine, from what the registry says. The `line` of
/// each is left empty.
///
/// A com0com port is listed under the name it is opened by, with the
/// driver's name for the other end of its pair, the name applications see
/// for that other end, and any modem attached there. A RustModem port is
/// one port with one name, which the service and the host both open, each
/// its own side: it is listed with its modem and no host port, so that
/// its binding is the modem's name. Every other port in the device map is
/// listed as it is, with any modem attached to it.
pub fn inventory(raw: &Raw) -> Vec<PortInfo> {
    // Windows port names do not distinguish case.
    let device_on = |port: &str| {
        raw.modems
            .iter()
            .find(|modem| modem.attached_to.eq_ignore_ascii_case(port))
    };
    let modem_on = |port: &str| device_on(port).map(|modem| modem.name.clone());
    let mut ports: Vec<PortInfo> = Vec::new();
    for port in &raw.com0com {
        let other = port.pair().and_then(|pair| {
            raw.com0com
                .iter()
                .find(|other| other.driver_name.eq_ignore_ascii_case(&pair))
        });
        let host_port = other.map(|other| other.name().to_string());
        ports.push(PortInfo {
            name: port.name().to_string(),
            kind: PortKind::Com0com,
            pair: other.map(|other| other.driver_name.clone()),
            modem: host_port.as_deref().and_then(modem_on),
            driver_version: None,
            host_port,
            line: None,
        });
    }
    for name in &raw.vcom {
        ports.push(PortInfo {
            name: name.clone(),
            kind: PortKind::Vcom,
            pair: None,
            host_port: None,
            modem: modem_on(name),
            driver_version: device_on(name).and_then(|modem| modem.driver_version.clone()),
            line: None,
        });
    }
    for name in &raw.serialcomm {
        if ports
            .iter()
            .any(|port| port.name.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let digits = name
            .get(3..)
            .filter(|_| name[..3].eq_ignore_ascii_case("COM"));
        let is_com = digits.is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        ports.push(PortInfo {
            name: name.clone(),
            kind: if is_com {
                PortKind::Serial
            } else {
                PortKind::Other
            },
            pair: None,
            host_port: None,
            modem: modem_on(name),
            driver_version: None,
            line: None,
        });
    }
    ports
}

/// The ports of this machine, read from the registry each time it is
/// asked.
#[cfg(windows)]
pub struct SystemPorts;

#[cfg(windows)]
impl ctlsrv::PortInventory for SystemPorts {
    fn ports(&self) -> Vec<PortInfo> {
        inventory(&read())
    }
}

/// The device map of serial ports: one value for each, whose data is the
/// port name.
#[cfg(windows)]
const SERIALCOMM: &str = r"HARDWARE\DEVICEMAP\SERIALCOMM";

/// The com0com driver's parameters: one subkey for each port, named as the
/// driver names the port, with the `PortName` value.
#[cfg(windows)]
const COM0COM_PARAMETERS: &str = r"SYSTEM\CurrentControlSet\Services\com0com\Parameters";

/// Where the device of a com0com port keeps the name the Ports class
/// installer chose for it, under a subkey named as the driver names the
/// port.
#[cfg(windows)]
const COM0COM_DEVICES: &str = r"SYSTEM\CurrentControlSet\Enum\COM0COM\PORT";

/// The Modem device class: one numbered subkey for each modem, with its
/// `FriendlyName` and the `AttachedTo` port.
#[cfg(windows)]
const MODEM_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4D36E96D-E325-11CE-BFC1-08002BE10318}";

/// Reads what the registry says. A key that is not there, as on a machine
/// with no com0com or no modem, gives nothing and is not an error.
#[cfg(windows)]
fn read() -> Raw {
    use crate::registry::Key;

    let mut raw = Raw::default();
    if let Some(map) = Key::open(SERIALCOMM) {
        for (device, port) in map.strings() {
            // The driver's own name for its entry: driver/vcom.
            if device.starts_with(r"\Device\RustModem") {
                raw.vcom.push(port.clone());
            }
            raw.serialcomm.push(port);
        }
    }
    if let Some(parameters) = Key::open(COM0COM_PARAMETERS) {
        let devices = Key::open(COM0COM_DEVICES);
        for driver_name in parameters.subkeys() {
            let Some(port) = parameters.subkey(&driver_name) else {
                continue;
            };
            let real_name = devices
                .as_ref()
                .and_then(|devices| devices.subkey(&driver_name))
                .and_then(|device| device.subkey("Device Parameters"))
                .and_then(|device| device.string("PortName"));
            raw.com0com.push(Com0comPort {
                port_name: port.string("PortName").unwrap_or_default(),
                driver_name,
                real_name,
            });
        }
    }
    if let Some(class) = Key::open(MODEM_CLASS) {
        for number in class.subkeys() {
            let Some(modem) = class.subkey(&number) else {
                continue;
            };
            let Some(attached_to) = modem.string("AttachedTo") else {
                continue;
            };
            let name = modem
                .string("FriendlyName")
                .or_else(|| modem.string("DriverDesc"))
                .unwrap_or_else(|| "Modem".to_string());
            raw.modems.push(ModemDevice {
                name,
                attached_to,
                driver_version: modem.string("DriverVersion"),
            });
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn com0com(driver_name: &str, port_name: &str, real_name: Option<&str>) -> Com0comPort {
        Com0comPort {
            driver_name: driver_name.to_string(),
            port_name: port_name.to_string(),
            real_name: real_name.map(str::to_string),
        }
    }

    fn modem(name: &str, attached_to: &str) -> ModemDevice {
        ModemDevice {
            name: name.to_string(),
            attached_to: attached_to.to_string(),
            driver_version: None,
        }
    }

    fn find<'a>(ports: &'a [PortInfo], name: &str) -> &'a PortInfo {
        ports
            .iter()
            .find(|port| port.name == name)
            .unwrap_or_else(|| panic!("no port {name} in {ports:?}"))
    }

    #[test]
    fn a_com0com_port_is_opened_by_its_port_name_parameter() {
        assert_eq!(com0com("CNCA0", "", None).name(), "CNCA0");
        assert_eq!(com0com("CNCA0", "-", None).name(), "CNCA0");
        assert_eq!(com0com("CNCA0", "COM8", None).name(), "COM8");
        assert_eq!(com0com("CNCA0", "COM#", Some("COM5")).name(), "COM5");
        // The installer has not chosen yet.
        assert_eq!(com0com("CNCA0", "COM#", None).name(), "CNCA0");
        assert_eq!(
            com0com("CNCA12", "", None).pair().as_deref(),
            Some("CNCB12")
        );
        assert_eq!(com0com("cncb3", "", None).pair().as_deref(), Some("CNCA3"));
        assert_eq!(com0com("OTHER", "", None).pair(), None);
    }

    #[test]
    fn the_set_up_of_the_com0com_document_is_two_pairs_with_host_ports() {
        // Two pairs as docs/COM0COM.md sets them up: the A side named by
        // the Ports class installer, the B side for the service, and a
        // standard modem installed on the first host port.
        let raw = Raw {
            serialcomm: ["COM1", "COM5", "CNCB0", "COM6", "CNCB1"]
                .map(String::from)
                .to_vec(),
            com0com: vec![
                com0com("CNCA0", "COM#", Some("COM5")),
                com0com("CNCB0", "-", None),
                com0com("CNCA1", "COM#", Some("COM6")),
                com0com("CNCB1", "", None),
            ],
            modems: vec![modem("Standard 56000 bps Modem", "com5")],
            ..Raw::default()
        };
        let ports = inventory(&raw);
        assert_eq!(ports.len(), 5, "{ports:?}");

        let line_side = find(&ports, "CNCB0");
        assert_eq!(line_side.kind, PortKind::Com0com);
        assert_eq!(line_side.pair.as_deref(), Some("CNCA0"));
        assert_eq!(line_side.host_port.as_deref(), Some("COM5"));
        assert_eq!(line_side.modem.as_deref(), Some("Standard 56000 bps Modem"));
        assert_eq!(
            line_side.binding().as_deref(),
            Some("Standard 56000 bps Modem on COM5")
        );
        assert_eq!(line_side.line, None);

        let second = find(&ports, "CNCB1");
        assert_eq!(second.host_port.as_deref(), Some("COM6"));
        assert_eq!(second.modem, None);
        assert_eq!(second.binding().as_deref(), Some("COM6"));

        // The host side is a com0com port too, seen from its own end.
        let host_side = find(&ports, "COM5");
        assert_eq!(host_side.kind, PortKind::Com0com);
        assert_eq!(host_side.pair.as_deref(), Some("CNCB0"));
        assert_eq!(host_side.host_port.as_deref(), Some("CNCB0"));
        assert_eq!(host_side.modem, None);

        let real = find(&ports, "COM1");
        assert_eq!(real.kind, PortKind::Serial);
        assert_eq!(
            (&real.pair, &real.host_port, &real.modem),
            (&None, &None, &None)
        );
        assert_eq!(real.binding(), None);
    }

    #[test]
    fn ports_that_are_not_com0com_are_serial_or_other() {
        let raw = Raw {
            serialcomm: ["COM3", "com12", "COMX", "COM", "BthModem0", "CO"]
                .map(String::from)
                .to_vec(),
            com0com: Vec::new(),
            modems: vec![modem("A real modem", "COM3")],
            ..Raw::default()
        };
        let ports = inventory(&raw);
        let kinds: Vec<(&str, PortKind)> =
            ports.iter().map(|p| (p.name.as_str(), p.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("COM3", PortKind::Serial),
                ("com12", PortKind::Serial),
                ("COMX", PortKind::Other),
                ("COM", PortKind::Other),
                ("BthModem0", PortKind::Other),
                ("CO", PortKind::Other),
            ]
        );
        // A modem on a real port is the binding of that port.
        assert_eq!(ports[0].binding().as_deref(), Some("A real modem"));
    }

    #[test]
    fn half_a_pair_and_a_port_missing_from_the_device_map_are_still_listed() {
        let raw = Raw {
            serialcomm: Vec::new(),
            com0com: vec![com0com("CNCB7", "", None)],
            modems: Vec::new(),
            ..Raw::default()
        };
        let ports = inventory(&raw);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, "CNCB7");
        assert_eq!((&ports[0].pair, &ports[0].host_port), (&None, &None));
        assert!(inventory(&Raw::default()).is_empty());
    }

    /// What this machine has is not known to the test; reading it must not
    /// fail, and what it gives must be consistent.
    #[cfg(windows)]
    #[test]
    fn this_machine_can_be_read() {
        use ctlsrv::PortInventory;
        let ports = SystemPorts.ports();
        for port in &ports {
            assert!(!port.name.is_empty());
            assert_eq!(port.line, None);
            if port.kind != PortKind::Com0com {
                assert_eq!((&port.pair, &port.host_port), (&None, &None));
            }
        }
    }

    #[test]
    fn a_rustmodem_port_is_one_name_with_its_modem_and_the_drivers_version() {
        // Two RustModem ports, one with its modem and one whose modem is
        // gone, beside a real port.
        let raw = Raw {
            serialcomm: ["COM1", "COM10", "COM11"].map(String::from).to_vec(),
            vcom: ["COM10", "COM11"].map(String::from).to_vec(),
            com0com: Vec::new(),
            modems: vec![ModemDevice {
                name: "RustModem Softmodem".to_string(),
                attached_to: "com10".to_string(),
                driver_version: Some("0.2.0.0".to_string()),
            }],
        };
        let ports = inventory(&raw);
        assert_eq!(ports.len(), 3, "each port once: {ports:?}");

        let first = find(&ports, "COM10");
        assert_eq!(first.kind, PortKind::Vcom);
        assert_eq!(first.modem.as_deref(), Some("RustModem Softmodem"));
        assert_eq!(first.driver_version.as_deref(), Some("0.2.0.0"));
        assert_eq!((&first.pair, &first.host_port), (&None, &None));
        // The binding of a line on it is the modem's name: the port's
        // name is the line's own.
        assert_eq!(first.binding().as_deref(), Some("RustModem Softmodem"));

        let second = find(&ports, "COM11");
        assert_eq!(second.kind, PortKind::Vcom);
        assert_eq!((&second.modem, &second.driver_version), (&None, &None));
        assert_eq!(second.binding(), None);

        assert_eq!(find(&ports, "COM1").kind, PortKind::Serial);
    }
}
