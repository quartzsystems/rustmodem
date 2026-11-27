// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `port add`, `port remove` and `port list`, against a device layer
//! that is a table: what is made, in what order, and what is taken away
//! again when a port cannot be finished.

use setup::ports::{self, Device, Devices};

/// The devices of a machine, and the COM numbers it has as taken.
#[derive(Default)]
struct Table {
    driver: bool,
    ports: Vec<Device>,
    modems: Vec<Device>,
    taken: Vec<u32>,
    next_instance: u32,
    /// The COM name whose modem cannot be made.
    modem_fails_for: Option<String>,
    /// Instances that cannot be removed.
    stuck: Vec<String>,
    /// What was done, in order.
    journal: Vec<String>,
}

impl Table {
    fn with_driver() -> Table {
        Table {
            driver: true,
            // A real port and a USB adapter, as a machine has.
            taken: vec![1, 3],
            ..Table::default()
        }
    }

    fn device(&mut self, kind: &str, com: &str) -> Device {
        let instance = format!("ROOT\\{kind}\\{:04}", self.next_instance);
        self.next_instance += 1;
        Device {
            instance,
            name: if kind == "PORTS" {
                format!("RustModem Port ({com})")
            } else {
                "RustModem Softmodem".to_string()
            },
            com: Some(com.to_string()),
            driver_version: Some("0.2.0.0".to_string()),
            started: true,
        }
    }
}

impl Devices for Table {
    fn driver_installed(&mut self) -> bool {
        self.driver
    }

    fn ports(&mut self) -> Result<Vec<Device>, String> {
        Ok(self.ports.clone())
    }

    fn modems(&mut self) -> Result<Vec<Device>, String> {
        Ok(self.modems.clone())
    }

    fn taken(&mut self) -> Result<Vec<u32>, String> {
        Ok(self.taken.clone())
    }

    fn claim(&mut self, number: u32) -> Result<bool, String> {
        self.journal.push(format!("claim {number}"));
        if self.taken.contains(&number) {
            return Ok(false);
        }
        self.taken.push(number);
        Ok(true)
    }

    fn release(&mut self, number: u32) -> Result<(), String> {
        self.journal.push(format!("release {number}"));
        self.taken.retain(|n| *n != number);
        Ok(())
    }

    fn create_port(&mut self, com: &str) -> Result<String, String> {
        self.journal.push(format!("port {com}"));
        let device = self.device("PORTS", com);
        self.ports.push(device.clone());
        Ok(device.instance)
    }

    fn create_modem(&mut self, com: &str) -> Result<String, String> {
        self.journal.push(format!("modem {com}"));
        if self.modem_fails_for.as_deref() == Some(com) {
            return Err("Windows said no".to_string());
        }
        let device = self.device("MODEM", com);
        self.modems.push(device.clone());
        Ok(device.instance)
    }

    fn remove(&mut self, instance: &str) -> Result<(), String> {
        self.journal.push(format!("remove {instance}"));
        if self.stuck.iter().any(|s| s == instance) {
            return Err("the device is in use".to_string());
        }
        self.ports.retain(|d| d.instance != instance);
        self.modems.retain(|d| d.instance != instance);
        Ok(())
    }
}

fn coms(table: &mut Table) -> Vec<String> {
    ports::list(table)
        .unwrap()
        .into_iter()
        .map(|p| p.com)
        .collect()
}

#[test]
fn a_port_is_a_claimed_number_a_port_device_and_its_modem() {
    let mut table = Table::with_driver();
    assert_eq!(ports::add(&mut table, Some(12), 1).unwrap(), ["COM12"]);
    assert_eq!(table.journal, ["claim 12", "port COM12", "modem COM12"]);
    let listed = ports::list(&mut table).unwrap();
    assert_eq!(listed.len(), 1);
    let port = &listed[0];
    assert_eq!(port.com, "COM12");
    assert_eq!(port.port_instance, "ROOT\\PORTS\\0000");
    assert_eq!(port.modem_instance.as_deref(), Some("ROOT\\MODEM\\0001"));
    assert_eq!(port.modem_name.as_deref(), Some("RustModem Softmodem"));
    assert_eq!(port.driver_version.as_deref(), Some("0.2.0.0"));
    assert!(port.started);
    assert!(table.taken.contains(&12));
}

#[test]
fn with_no_number_given_ports_take_the_free_numbers_from_com10() {
    let mut table = Table::with_driver();
    table.taken.extend([10, 12]);
    assert_eq!(
        ports::add(&mut table, None, 3).unwrap(),
        ["COM11", "COM13", "COM14"]
    );
    // More ports go on from there, and a count from a number skips what
    // is taken.
    assert_eq!(ports::add(&mut table, None, 1).unwrap(), ["COM15"]);
    assert_eq!(
        ports::add(&mut table, Some(2), 3).unwrap(),
        ["COM2", "COM4", "COM5"]
    );
    assert_eq!(
        coms(&mut table),
        ["COM2", "COM4", "COM5", "COM11", "COM13", "COM14", "COM15"],
        "listed in the order of their numbers"
    );
}

#[test]
fn a_number_that_is_taken_or_is_no_number_is_refused_and_nothing_is_made() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(20), 1).unwrap();
    table.journal.clear();
    for number in [1, 3, 20, 0, 257] {
        let refused = ports::add(&mut table, Some(number), 1).unwrap_err();
        assert_eq!(refused.id, "RMD9006", "COM{number}");
    }
    assert_eq!(table.journal, [] as [&str; 0]);
    assert_eq!(coms(&mut table), ["COM20"]);
}

#[test]
fn with_no_driver_no_port_is_made() {
    let mut table = Table::default();
    assert_eq!(ports::add(&mut table, None, 1).unwrap_err().id, "RMD9010");
    assert_eq!(table.journal, [] as [&str; 0]);
}

#[test]
fn when_the_numbers_run_out_nothing_is_made() {
    let mut table = Table::with_driver();
    let refused = ports::add(&mut table, Some(255), 3).unwrap_err();
    assert_eq!(refused.id, "RMD9011");
    assert_eq!(table.journal, [] as [&str; 0]);
    assert_eq!(
        ports::add(&mut table, Some(255), 2).unwrap(),
        ["COM255", "COM256"]
    );
}

#[test]
fn a_port_that_cannot_be_finished_is_taken_away_with_those_before_it() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(30), 1).unwrap();
    table.journal.clear();
    table.modem_fails_for = Some("COM12".to_string());
    let failure = ports::add(&mut table, None, 4).unwrap_err();
    assert_eq!(failure.id, "RMD9008");
    assert!(failure.text.contains("COM12") && failure.text.contains("Windows said no"));
    assert_eq!(
        table.journal,
        [
            "claim 10",
            "port COM10",
            "modem COM10",
            "claim 11",
            "port COM11",
            "modem COM11",
            "claim 12",
            "port COM12",
            "modem COM12",
            // Undone last first, and of each port the modem first.
            "remove ROOT\\PORTS\\0006",
            "release 12",
            "remove ROOT\\MODEM\\0005",
            "remove ROOT\\PORTS\\0004",
            "release 11",
            "remove ROOT\\MODEM\\0003",
            "remove ROOT\\PORTS\\0002",
            "release 10",
        ]
    );
    // The port that was there before is as it was.
    assert_eq!(coms(&mut table), ["COM30"]);
    assert_eq!(table.taken, [1, 3, 30]);
}

#[test]
fn removing_a_port_removes_its_modem_first_and_frees_its_number() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(10), 2).unwrap();
    table.journal.clear();
    ports::remove(&mut table, "com10").unwrap();
    assert_eq!(
        table.journal,
        [
            "remove ROOT\\MODEM\\0001",
            "remove ROOT\\PORTS\\0000",
            "release 10"
        ]
    );
    assert_eq!(coms(&mut table), ["COM11"]);
    assert!(!table.taken.contains(&10) && table.taken.contains(&11));
}

#[test]
fn a_port_that_is_not_rustmodems_is_not_removed() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(10), 1).unwrap();
    table.journal.clear();
    // COM1 is the machine's own port.
    for name in ["COM1", "COM11"] {
        assert_eq!(
            ports::remove(&mut table, name).unwrap_err().id,
            "RMD9001",
            "{name}"
        );
    }
    assert_eq!(table.journal, [] as [&str; 0]);
}

#[test]
fn a_port_that_is_in_use_is_said_not_to_have_been_removed() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(10), 1).unwrap();
    table.stuck.push("ROOT\\PORTS\\0000".to_string());
    let failure = ports::remove(&mut table, "COM10").unwrap_err();
    assert_eq!(failure.id, "RMD9009");
    assert!(failure.text.contains("in use"));
    // Its number is still its own.
    assert!(table.taken.contains(&10));
}

#[test]
fn a_port_with_no_modem_is_listed_as_not_started() {
    let mut table = Table::with_driver();
    ports::add(&mut table, Some(10), 1).unwrap();
    table.modems.clear();
    let listed = ports::list(&mut table).unwrap();
    assert_eq!(listed[0].modem_instance, None);
    assert!(!listed[0].started);
    // And it is still removed as a port.
    ports::remove(&mut table, "COM10").unwrap();
    assert_eq!(coms(&mut table), [] as [&str; 0]);
}

#[test]
fn an_uninstall_removes_every_port_and_any_modem_left_alone() {
    let mut table = Table::with_driver();
    ports::add(&mut table, None, 3).unwrap();
    // A modem whose port has gone.
    let orphan = table.device("MODEM", "COM99");
    table.modems.push(orphan);
    ports::remove_all(&mut table).unwrap();
    assert_eq!(coms(&mut table), [] as [&str; 0]);
    assert!(table.modems.is_empty());
    assert_eq!(table.taken, [1, 3]);

    // One that is stuck is said, and the others still go.
    let mut table = Table::with_driver();
    ports::add(&mut table, None, 2).unwrap();
    table.stuck.push("ROOT\\PORTS\\0000".to_string());
    let failure = ports::remove_all(&mut table).unwrap_err();
    assert!(failure.contains("COM10") && failure.contains("in use"));
    assert_eq!(coms(&mut table), ["COM10"]);
}

#[test]
fn a_com_name_is_com_and_a_number_from_1_to_256() {
    for (name, number) in [("COM1", 1), ("com10", 10), ("Com256", 256)] {
        assert_eq!(ports::com_number(name), Some(number), "{name}");
    }
    for name in [
        "COM0", "COM257", "COM", "COM01", "COMX", "CNCB0", "", "COM 3", "COM3 ",
    ] {
        assert_eq!(ports::com_number(name), None, "{name:?}");
    }
}
