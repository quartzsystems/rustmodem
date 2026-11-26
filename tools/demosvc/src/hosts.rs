// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The memory ports of the demo service: the pool's end of each, and the
//! host's end, which the scripts play.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use config::LineConfig;
use ctlproto::{PortInfo, PortKind};
use msg::{Message, message};
use pool::SerialFactory;
use serial::{MemoryHost, SerialBackend, memory_pair};

/// How many com0com pairs the demo machine pretends to have beyond the
/// ones its lines use, so that a line can be added on a free port.
const SPARE_PAIRS: u16 = 4;

/// The modem the demo machine pretends to have installed on its host
/// ports.
const MODEM: &str = "Standard 56000 bps Modem";

#[derive(Default)]
struct Inner {
    /// The host's end of every port the pool has opened, by port name.
    hosts: BTreeMap<String, MemoryHost>,
    /// Ports that will not open.
    refused: BTreeSet<String>,
}

/// The host's end of every port, shared between the pool's serial factory
/// and the scripts.
#[derive(Clone, Default)]
pub struct Hosts(Arc<Mutex<Inner>>);

impl Hosts {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The host's end of a port, once the pool has opened it.
    pub fn get(&self, port: &str) -> Option<MemoryHost> {
        self.lock().hosts.get(port).cloned()
    }

    /// Makes a port refuse to open, as a port that does not exist does.
    pub fn refuse(&self, port: &str) {
        self.lock().refused.insert(port.to_string());
    }

    /// What opens the lines' ports.
    pub fn factory(&self) -> Box<dyn SerialFactory> {
        Box::new(Factory(self.clone()))
    }
}

struct Factory(Hosts);

impl SerialFactory for Factory {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let mut inner = self.0.lock();
        let port = &line.serial.port;
        if inner.refused.contains(port) {
            // What Windows says of a port that is not there.
            return Err(message(
                "RMD3001",
                &[
                    port,
                    "The system cannot find the file specified. (os error 2)",
                ],
            ));
        }
        let (backend, host) = memory_pair();
        inner.hosts.insert(port.clone(), host);
        Ok(Box::new(backend))
    }
}

/// The name of the port line `id` is on when the demo service starts.
pub fn port_of_line(id: u16) -> String {
    format!("CNCB{}", id - 1)
}

/// The serial ports the demo machine pretends to have: one real port, and
/// a com0com pair for each line and a few to spare. Most host ports have a
/// modem installed on them; every fourth has none, to show a line bound to
/// a bare port.
pub fn demo_ports(lines: u16) -> Vec<PortInfo> {
    let mut ports = vec![PortInfo {
        name: "COM1".to_string(),
        kind: PortKind::Serial,
        pair: None,
        host_port: None,
        modem: None,
        line: None,
    }];
    for pair in 0..lines + SPARE_PAIRS {
        let host_port = format!("COM{}", 11 + pair);
        let has_modem = pair < lines && pair % 4 != 3;
        // The service's end of the pair, which a line opens.
        ports.push(PortInfo {
            name: format!("CNCB{pair}"),
            kind: PortKind::Com0com,
            pair: Some(format!("CNCA{pair}")),
            host_port: Some(host_port.clone()),
            modem: has_modem.then(|| MODEM.to_string()),
            line: None,
        });
        // The host's end, as an application sees it.
        ports.push(PortInfo {
            name: host_port,
            kind: PortKind::Com0com,
            pair: Some(format!("CNCB{pair}")),
            host_port: Some(format!("CNCB{pair}")),
            modem: None,
            line: None,
        });
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_has_a_pair_and_there_are_some_to_spare() {
        let ports = demo_ports(4);
        assert_eq!(ports.len(), 1 + 2 * 8);
        for line in 1..=4 {
            let name = port_of_line(line);
            let port = ports.iter().find(|p| p.name == name).unwrap();
            assert_eq!(port.kind, PortKind::Com0com);
            assert_eq!(port.pair, Some(format!("CNCA{}", line - 1)));
            assert_eq!(port.host_port, Some(format!("COM{}", 10 + line)));
        }
        let binding = |name: &str| ports.iter().find(|p| p.name == name).unwrap().binding();
        assert_eq!(
            binding("CNCB0").as_deref(),
            Some("Standard 56000 bps Modem on COM11")
        );
        assert_eq!(binding("CNCB3").as_deref(), Some("COM14"));
        // A spare pair has no modem yet.
        assert_eq!(binding("CNCB5").as_deref(), Some("COM16"));
        assert_eq!(binding("COM1"), None);
    }

    #[test]
    fn a_port_opens_unless_it_is_refused() {
        let hosts = Hosts::default();
        let mut factory = hosts.factory();
        assert!(hosts.get("CNCB0").is_none());
        assert!(factory.open(&LineConfig::new(1, "CNCB0")).is_ok());
        assert!(hosts.get("CNCB0").is_some());
        hosts.refuse("CNCB7");
        let Err(problem) = factory.open(&LineConfig::new(8, "CNCB7")) else {
            panic!("the port is refused");
        };
        assert_eq!(problem.id, "RMD3001");
        assert!(hosts.get("CNCB7").is_none());
    }
}
