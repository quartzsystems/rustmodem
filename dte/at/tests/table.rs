// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! One case for each row of the command table and of the register table in
//! the phase 0 handoff. Each case is a small transcript (see `common`).

mod common;

/// A table row and the transcript that exercises it. Every transcript
/// starts with echo turned off, so the expectations show only responses.
struct Row {
    row: &'static str,
    script: &'static str,
}

const ECHO_OFF: &str = "> ATE0\\r\n< ATE0\\r\\r\\nOK\\r\\n\n";

/// Brings a call up and escapes to online command mode.
const ONLINE_COMMAND: &str = "\
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ elapsed 1000
> +++
! line +++
@ elapsed 1000
< \\r\\nOK\\r\\n
! mode onlinecommand
";

const COMMANDS: &[Row] = &[
    Row {
        row: "A",
        script: "\
> ATA\\r
! answer
! mode handshaking
@ connected
< \\r\\nCONNECT\\r\\n
! mode online
",
    },
    Row {
        row: "A is refused during a call",
        script: "\
$ONLINE_COMMAND
> ATA\\r
< \\r\\nERROR\\r\\n
! mode onlinecommand
",
    },
    Row {
        row: "D keeps digits * # + , W and drops the rest",
        script: "\
> ATDT 9,(555) 01-23*#+w.x\\r
! dial 9,5550123*#+W
! mode handshaking
",
    },
    Row {
        row: "DP and a bare D",
        script: "\
> ATDP12\\r
! dial 12
@ busy
< \\r\\nBUSY\\r\\n
> ATD\\r
! dial
! mode handshaking
",
    },
    Row {
        row: "D with a trailing ; returns to command mode",
        script: "\
> ATD5;\\r
! dial 5
< \\r\\nOK\\r\\n
! mode command
> AT\\r
< \\r\\nOK\\r\\n
@ connected
! mode onlinecommand
> ATH\\r
! hangup command
< \\r\\nOK\\r\\n
! mode command
",
    },
    Row {
        row: "D is refused during a call",
        script: "\
$ONLINE_COMMAND
> ATD6\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "E0 E1",
        script: "\
> ATE1\\r
< \\r\\nOK\\r\\n
> AT\\r
< AT\\r\\r\\nOK\\r\\n
> ATE0\\r
< ATE0\\r\\r\\nOK\\r\\n
> AT\\r
< \\r\\nOK\\r\\n
> ATE\\r
< \\r\\nOK\\r\\n
> ATE2\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "H H0 H1",
        script: "\
> ATH\\r
< \\r\\nOK\\r\\n
> ATH0\\r
< \\r\\nOK\\r\\n
> ATH1\\r
< \\r\\nOK\\r\\n
> ATH2\\r
< \\r\\nERROR\\r\\n
$ONLINE_COMMAND
> ATH1\\r
< \\r\\nOK\\r\\n
! mode onlinecommand
> ATH\\r
! hangup command
< \\r\\nOK\\r\\n
! mode command
! signals dcd=0
",
    },
    Row {
        row: "I I0..I9",
        script: "\
> ATI\\r
< \\r\\nrustmodem\\r\\n\\r\\nOK\\r\\n
> ATI0\\r
< \\r\\nrustmodem\\r\\n\\r\\nOK\\r\\n
> ATI3\\r
< \\r\\nRustModem ${VERSION}\\r\\n\\r\\nOK\\r\\n
> ATI4\\r
< \\r\\nNULL MODEM,V.250,+MS,+ES,+DS\\r\\n\\r\\nOK\\r\\n
> ATI1\\r
< \\r\\n000\\r\\n\\r\\nOK\\r\\n
> ATI5\\r
< \\r\\nQuartz Systems\\r\\n\\r\\nOK\\r\\n
> ATI6\\r
< \\r\\nRustModem Softmodem\\r\\n\\r\\nOK\\r\\n
> ATI7\\r
< \\r\\nRustModem Softmodem ${VERSION}\\r\\n\\r\\nOK\\r\\n
> ATI2I8I9\\r
< \\r\\nOK\\r\\n
> ATI10\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "+GMI +GMM +GMR +GSN +GCAP",
        script: "\
> AT+GMI\\r
< \\r\\nQuartz Systems\\r\\n\\r\\nOK\\r\\n
> AT+GMM\\r
< \\r\\nRustModem Softmodem\\r\\n\\r\\nOK\\r\\n
> AT+GMR\\r
< \\r\\n${VERSION}\\r\\n\\r\\nOK\\r\\n
> AT+GSN\\r
< \\r\\n00000000\\r\\n\\r\\nOK\\r\\n
> AT+GCAP\\r
< \\r\\n+GCAP: +MS,+ES,+DS\\r\\n\\r\\nOK\\r\\n
> AT+GMI;+GMM\\r
< \\r\\nQuartz Systems\\r\\n\\r\\nRustModem Softmodem\\r\\n\\r\\nOK\\r\\n
> AT+GMI=?;+GMM=?;+GMR=?;+GSN=?;+GCAP=?\\r
< \\r\\nOK\\r\\n
> AT+GMI?\\r
< \\r\\nERROR\\r\\n
> AT+GMM=1\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "+FCLASS #CLS",
        script: "\
> AT+FCLASS=?\\r
< \\r\\n0\\r\\n\\r\\nOK\\r\\n
> AT+FCLASS?\\r
< \\r\\n0\\r\\n\\r\\nOK\\r\\n
> AT+FCLASS=0\\r
< \\r\\nOK\\r\\n
> AT+FCLASS=1\\r
< \\r\\nERROR\\r\\n
> AT+FCLASS=8\\r
< \\r\\nERROR\\r\\n
> AT#CLS=?\\r
< \\r\\n0\\r\\n\\r\\nOK\\r\\n
> AT#CLS?\\r
< \\r\\n0\\r\\n\\r\\nOK\\r\\n
> AT#CLS=0\\r
< \\r\\nOK\\r\\n
> AT#CLS=8\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "+GCI",
        script: "\
> AT+GCI?\\r
< \\r\\n+GCI: B5\\r\\n\\r\\nOK\\r\\n
> AT+GCI=?\\r
< \\r\\n+GCI: (00-FF)\\r\\n\\r\\nOK\\r\\n
> AT+GCI=b4\\r
< \\r\\nOK\\r\\n
> AT+GCI?\\r
< \\r\\n+GCI: B4\\r\\n\\r\\nOK\\r\\n
> AT+GCI=0\\r
< \\r\\nOK\\r\\n
> AT+GCI?\\r
< \\r\\n+GCI: 00\\r\\n\\r\\nOK\\r\\n
> AT+GCI=100\\r
< \\r\\nERROR\\r\\n
> AT+GCI=ZZ\\r
< \\r\\nERROR\\r\\n
> AT+GCI\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "L M",
        script: "\
> ATL\\r
< \\r\\nOK\\r\\n
> ATL3M2\\r
< \\r\\nOK\\r\\n
> AT&V\\r
< \\r\\nACTIVE PROFILE:\\r\\nE0 Q0 V1 X4 L3 M2 &C1 &D2 &K3\\r\\n
< S00:000 S01:000 S02:043 S03:013 S04:010 S05:008 S06:002 S07:050 S09:006 S10:014 S12:050 S36:007 S46:138 S48:007\\r\\n
< +MS: V22B,1,0,0,0,0\\r\\n+ES: 3,0,2\\r\\n+DS: 3,0,2048,32\\r\\n+ICF: 3,3\\r\\n+MR: 0 +ER: 0 +DR: 0 +ILRR: 0\\r\\n\\r\\nOK\\r\\n
> ATL4\\r
< \\r\\nERROR\\r\\n
> ATM4\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "O O0",
        script: "\
> ATO\\r
< \\r\\nERROR\\r\\n
$ONLINE_COMMAND
> ATO1\\r
< \\r\\nERROR\\r\\n
> ATO\\r
< \\r\\nCONNECT\\r\\n
! mode online
> x
! line x
@ elapsed 1000
> +++
! line +++
@ elapsed 1000
< \\r\\nOK\\r\\n
> ATO0\\r
< \\r\\nCONNECT\\r\\n
! mode online
",
    },
    Row {
        row: "Q0 Q1",
        script: "\
> ATQ1\\r
> AT\\r
> ATI0\\r
< \\r\\nrustmodem\\r\\n
> ATQ2\\r
> ATQ0\\r
< \\r\\nOK\\r\\n
> ATQ\\r
< \\r\\nOK\\r\\n
",
    },
    Row {
        row: "S<n>=<v> S<n>?",
        script: "\
> ATS0=2\\r
< \\r\\nOK\\r\\n
> ATS0?\\r
< \\r\\n002\\r\\n\\r\\nOK\\r\\n
> ATS7=60S7?S0?\\r
< \\r\\n060\\r\\n\\r\\n002\\r\\n\\r\\nOK\\r\\n
> ATS0=\\r
< \\r\\nOK\\r\\n
! s 0=0
> ATS0=256\\r
< \\r\\nERROR\\r\\n
> ATS=1\\r
< \\r\\nERROR\\r\\n
> ATS0\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "V0 V1",
        script: "\
> ATV0\\r
< 0\\r
> AT\\r
< 0\\r
> ATI0\\r
< rustmodem\\r\\n0\\r
> ATH2\\r
< 4\\r
> ATV1\\r
< \\r\\nOK\\r\\n
> ATV\\r
< 0\\r
> ATV2\\r
< 4\\r
",
    },
    Row {
        row: "X0: bare CONNECT, no BUSY or NO DIALTONE",
        script: "\
> ATX0\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ busy
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ nodialtone
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ noanswer
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ connected 2400
< \\r\\nCONNECT\\r\\n
",
    },
    Row {
        row: "X1 X2 X3",
        script: "\
> ATX1\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ busy
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ nodialtone
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ noanswer
< \\r\\nNO ANSWER\\r\\n
> ATX2\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ busy
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ nodialtone
< \\r\\nNO DIALTONE\\r\\n
> ATX3\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ busy
< \\r\\nBUSY\\r\\n
> ATD5\\r
! dial 5
@ nodialtone
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ connected 2400
< \\r\\nCONNECT 2400\\r\\n
",
    },
    Row {
        row: "X4: all codes, CONNECT <rate>",
        script: "\
> ATX\\r
< \\r\\nOK\\r\\n
> ATX4\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ busy
< \\r\\nBUSY\\r\\n
> ATD5\\r
! dial 5
@ nodialtone
< \\r\\nNO DIALTONE\\r\\n
> ATD5\\r
! dial 5
@ noanswer
< \\r\\nNO ANSWER\\r\\n
> ATD5\\r
! dial 5
@ connected 14400
< \\r\\nCONNECT 14400\\r\\n
@ carrierlost
< \\r\\nNO CARRIER\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ carrierlost
< \\r\\nNO CARRIER\\r\\n
> ATX5\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "Z Z0",
        script: "\
> ATS0=3X1\\r
< \\r\\nOK\\r\\n
> ATZ E0\\r
< \\r\\nOK\\r\\n
! s 0=0
> ATS0?\\r
< ATS0?\\r\\r\\n000\\r\\n\\r\\nOK\\r\\n
> ATZ0\\r
< ATZ0\\r\\r\\nOK\\r\\n
> ATZ1\\r
< ATZ1\\r\\r\\nERROR\\r\\n
",
    },
    Row {
        row: "Z hangs up",
        script: "\
$ONLINE_COMMAND
> ATZ\\r
! hangup reset
< \\r\\nOK\\r\\n
! mode command
! signals dcd=0
",
    },
    Row {
        row: "&F",
        script: "\
> ATS0=3S7=10X1V0&FS0?S7?\\r
< \\r\\n000\\r\\n\\r\\n050\\r\\n\\r\\nOK\\r\\n
> AT\\r
< AT\\r\\r\\nOK\\r\\n
> AT&F0\\r
< AT&F0\\r\\r\\nOK\\r\\n
> AT&F1\\r
< AT&F1\\r\\r\\nERROR\\r\\n
",
    },
    Row {
        row: "&F does not hang up",
        script: "\
$ONLINE_COMMAND
> AT&F\\r
< \\r\\nOK\\r\\n
! mode onlinecommand
",
    },
    Row {
        row: "&C0 &C1",
        script: "\
! signals dcd=0 dsr=1 cts=1 ri=0
> AT&C0\\r
< \\r\\nOK\\r\\n
! signals dcd=1
> AT&C1\\r
< \\r\\nOK\\r\\n
! signals dcd=0
> AT&C\\r
< \\r\\nOK\\r\\n
! signals dcd=1
> AT&C1\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
! signals dcd=0
@ connected
< \\r\\nCONNECT\\r\\n
! signals dcd=1
@ carrierlost
< \\r\\nNO CARRIER\\r\\n
! signals dcd=0
> AT&C2\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "&D0: DTR is ignored",
        script: "\
> AT&D0\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ dtr off
! mode online
@ dtr on
! mode online
",
    },
    Row {
        row: "&D1: DTR drop goes to online command",
        script: "\
> AT&D1\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ dtr off
< \\r\\nOK\\r\\n
! mode onlinecommand
! signals dcd=1
",
    },
    Row {
        row: "&D2: DTR drop hangs up",
        script: "\
> AT&D2S0=4\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ dtr off
! hangup dtr
< \\r\\nOK\\r\\n
! mode command
! signals dcd=0
! s 0=4
",
    },
    Row {
        row: "&D3: DTR drop hangs up and resets",
        script: "\
> AT&D3S0=4\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ dtr off
! hangup dtr
< \\r\\nOK\\r\\n
! mode command
! s 0=0
> AT&D4\\r
< AT&D4\\r\\r\\nERROR\\r\\n
",
    },
    Row {
        row: "&K0 &K3",
        script: "\
> AT&K0\\r
< \\r\\nOK\\r\\n
> AT&V\\r
< \\r\\nACTIVE PROFILE:\\r\\nE0 Q0 V1 X4 L0 M0 &C1 &D2 &K0\\r\\n
< S00:000 S01:000 S02:043 S03:013 S04:010 S05:008 S06:002 S07:050 S09:006 S10:014 S12:050 S36:007 S46:138 S48:007\\r\\n
< +MS: V22B,1,0,0,0,0\\r\\n+ES: 3,0,2\\r\\n+DS: 3,0,2048,32\\r\\n+ICF: 3,3\\r\\n+MR: 0 +ER: 0 +DR: 0 +ILRR: 0\\r\\n\\r\\nOK\\r\\n
> AT&K3\\r
< \\r\\nOK\\r\\n
> AT&K\\r
< \\r\\nOK\\r\\n
> AT&K1\\r
< \\r\\nERROR\\r\\n
> AT&K4\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "&V",
        script: "\
> ATS0=2S7=45X3&D1&C0\\r
< \\r\\nOK\\r\\n
> AT+MS=B103,0,300,300;+ES=1,0,2;+DS=0,0,512,6\\r
< \\r\\nOK\\r\\n
> AT&V\\r
< \\r\\nACTIVE PROFILE:\\r\\nE0 Q0 V1 X3 L0 M0 &C0 &D1 &K3\\r\\n
< S00:002 S01:000 S02:043 S03:013 S04:010 S05:008 S06:002 S07:045 S09:006 S10:014 S12:050 S36:007 S46:138 S48:007\\r\\n
< +MS: B103,0,300,300,0,0\\r\\n+ES: 1,0,2\\r\\n+DS: 0,0,512,6\\r\\n+ICF: 3,3\\r\\n+MR: 0 +ER: 0 +DR: 0 +ILRR: 0\\r\\n\\r\\nOK\\r\\n
> AT&V1\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "&W",
        script: "\
> ATS0=3S7=20&W\\r
! store 0=3 7=20
< \\r\\nOK\\r\\n
> AT&W0&W1\\r
! store 0=3
! store 0=3
< \\r\\nOK\\r\\n
> AT&W2\\r
< \\r\\nERROR\\r\\n
> ATS0=0S7=50\\r
< \\r\\nOK\\r\\n
> ATZ\\r
< \\r\\nOK\\r\\n
! s 0=3
! s 7=20
",
    },
    Row {
        row: "+MS +ES +DS: set, read and test",
        script: "\
> AT+MS?\\r
< \\r\\n+MS: V22B,1,0,0,0,0\\r\\n\\r\\nOK\\r\\n
> AT+MS=b103,0,300,300\\r
< \\r\\nOK\\r\\n
> AT+MS?\\r
< \\r\\n+MS: B103,0,300,300,0,0\\r\\n\\r\\nOK\\r\\n
> AT+MS=?\\r
< \\r\\n+MS: (V21,V22,V22B,B103,B212A),(0,1),(0,300,1200,2400),(0,300,1200,2400),(0,300,1200,2400),(0,300,1200,2400)\\r\\n\\r\\nOK\\r\\n
> AT+ES?;+ES=?\\r
< \\r\\n+ES: 3,0,2\\r\\n\\r\\n+ES: (0-3),(0-3),(0-5)\\r\\n\\r\\nOK\\r\\n
> AT+ES=1,0,2;+DS=0,0,512,6;+ES?;+DS?;E0\\r
< \\r\\n+ES: 1,0,2\\r\\n\\r\\n+DS: 0,0,512,6\\r\\n\\r\\nOK\\r\\n
> AT+DS=?\\r
< \\r\\n+DS: (0-3),(0),(512-4096),(6-250)\\r\\n\\r\\nOK\\r\\n
> AT+DS=\\r
< \\r\\nOK\\r\\n
> AT+DS?\\r
< \\r\\n+DS: 3,0,2048,32\\r\\n\\r\\nOK\\r\\n
> AT+MS\\r
< \\r\\nERROR\\r\\n
> AT+MS=(1)\\r
< \\r\\nERROR\\r\\n
> AT+MS?junk\\r
< \\r\\nERROR\\r\\n
",
    },
    Row {
        row: "+ICF: character framing",
        script: "\
> AT+ICF?;+ICF=?\\r
< \\r\\n+ICF: 3,3\\r\\n\\r\\n+ICF: (3,5),(0-3)\\r\\n\\r\\nOK\\r\\n
> AT+ICF=5,1;+ICF?\\r
< \\r\\n+ICF: 5,1\\r\\n\\r\\nOK\\r\\n
> AT+ICF=,0;+ICF?\\r
< \\r\\n+ICF: 5,0\\r\\n\\r\\nOK\\r\\n
> AT+ICF=3;+ICF?\\r
< \\r\\n+ICF: 3,0\\r\\n\\r\\nOK\\r\\n
> AT+ICF=5,2\\r
< \\r\\nERROR\\r\\n
> AT+ICF=4\\r
< \\r\\nERROR\\r\\n
> AT+ICF=0\\r
< \\r\\nERROR\\r\\n
> AT+ICF=3,3,1\\r
< \\r\\nERROR\\r\\n
> AT+ICF?\\r
< \\r\\n+ICF: 3,0\\r\\n\\r\\nOK\\r\\n
> AT+ICF=;+ICF?\\r
< \\r\\n+ICF: 3,3\\r\\n\\r\\nOK\\r\\n
",
    },
    Row {
        row: "anything else, unknown_command = ok",
        script: "\
> ATB0N1\\\\N3%C1*Q2#CID=1-K0$B5)M1@M3:E0W2Y0\\r
< \\r\\nOK\\r\\n
> AT&Q5&S0&\\r
< \\r\\nOK\\r\\n
> AT+VCID=1;+VLS?;+IPR?\\r
< \\r\\nOK\\r\\n
> ATS30=0S95=47\\r
< \\r\\nOK\\r\\n
> ATS95?\\r
< \\r\\n000\\r\\n\\r\\nOK\\r\\n
> ATY0X5\\r
< \\r\\nERROR\\r\\n
",
    },
];

const UNKNOWN_IS_ERROR: &[Row] = &[Row {
    row: "anything else, unknown_command = error",
    script: "\
> ATY0\\r
< \\r\\nERROR\\r\\n
> ATX1Y0X2\\r
< \\r\\nERROR\\r\\n
> AT&V\\r
< \\r\\nACTIVE PROFILE:\\r\\nE0 Q0 V1 X1 L0 M0 &C1 &D2 &K3\\r\\n
< S00:000 S01:000 S02:043 S03:013 S04:010 S05:008 S06:002 S07:050 S09:006 S10:014 S12:050 S36:007 S46:138 S48:007\\r\\n
< +MS: V22B,1,0,0,0,0\\r\\n+ES: 3,0,2\\r\\n+DS: 3,0,2048,32\\r\\n+ICF: 3,3\\r\\n+MR: 0 +ER: 0 +DR: 0 +ILRR: 0\\r\\n\\r\\nOK\\r\\n
> AT\\\\Q3\\r
< \\r\\nERROR\\r\\n
> AT&S0\\r
< \\r\\nERROR\\r\\n
> AT+VCID=1\\r
< \\r\\nERROR\\r\\n
> AT#CID=1\\r
< \\r\\nERROR\\r\\n
> ATS30=0\\r
< \\r\\nERROR\\r\\n
> ATS30?\\r
< \\r\\nERROR\\r\\n
> ATE0S0=1&K3\\\\N3%C3&Q5+MS?\\r
< \\r\\n+MS: V22B,1,0,0,0,0\\r\\n\\r\\nOK\\r\\n
",
}];

const REGISTERS: &[Row] = &[
    Row {
        row: "S0: rings before auto-answer; 0 is off",
        script: "\
! s 0=0
@ ring
< \\r\\nRING\\r\\n
@ ringend
@ ring
< \\r\\nRING\\r\\n
! mode ringing
@ ringend
@ elapsed 8000
> ATS0=2\\r
< \\r\\nOK\\r\\n
@ ring
< \\r\\nRING\\r\\n
! mode ringing
@ ringend
@ ring
< \\r\\nRING\\r\\n
! answer
! mode handshaking
@ connected
< \\r\\nCONNECT\\r\\n
",
    },
    Row {
        row: "S1: ring count, reset 8 s after the last ring, read-only in effect",
        script: "\
! s 1=0
@ ring
< \\r\\nRING\\r\\n
! s 1=1
@ elapsed 6000
@ ring
< \\r\\nRING\\r\\n
! s 1=2
> ATS1?\\r
< \\r\\n002\\r\\n\\r\\nOK\\r\\n
@ elapsed 7999
! s 1=2
! mode ringing
@ elapsed 1
! s 1=0
! mode command
> ATS1=9\\r
< \\r\\nOK\\r\\n
! s 1=0
",
    },
    Row {
        row: "S2: escape character; above 127 disables the escape",
        script: "\
! s 2=43
> ATS2=35\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ elapsed 1000
> +++
! line +++
@ elapsed 1000
! mode online
> ###
! line ###
@ elapsed 1000
< \\r\\nOK\\r\\n
! mode onlinecommand
> ATS2=128\\r
< \\r\\nOK\\r\\n
> ATO\\r
< \\r\\nCONNECT\\r\\n
@ elapsed 1000
> ###
! line ###
@ elapsed 1000
> \\x80\\x80\\x80
! line \\x80\\x80\\x80
@ elapsed 1000
! mode online
",
    },
    Row {
        row: "S3: command line terminator",
        script: "\
! s 3=13
> ATS3=59\\r
< ;\\nOK;\\n
> AT\\r
> I0;
< ;\\nrustmodem;\\n;\\nOK;\\n
> ATS3=128;
< ;\\nERROR;\\n
> ATS3=13;
< \\r\\nOK\\r\\n
",
    },
    Row {
        row: "S4: response formatting character",
        script: "\
! s 4=10
> ATS4=33\\r
< \\r!OK\\r!
> ATI0\\r
< \\r!rustmodem\\r!\\r!OK\\r!
> ATV0\\r
< 0\\r
> ATI0\\r
< rustmodem\\r!0\\r
> ATS4=128\\r
< 4\\r
",
    },
    Row {
        row: "S5: editing character",
        script: "\
! s 5=8
> ATE1\\r
< \\r\\nOK\\r\\n
> ATH2\\x08\\r
< ATH2\\x08 \\x08\\r\\r\\nOK\\r\\n
> AT\\x08\\x08\\r
< AT\\r\\r\\nOK\\r\\n
> ATS5=64\\r
< ATS5=64\\r\\r\\nOK\\r\\n
> ATH2@\\r
< ATH2@ @\\r\\r\\nOK\\r\\n
> ATH2\\x08\\r
< ATH2\\r\\r\\nERROR\\r\\n
",
    },
    Row {
        row: "S6: wait before dialling; stored",
        script: "\
! s 6=2
> ATS6=4S6?\\r
< \\r\\n004\\r\\n\\r\\nOK\\r\\n
! s 6=4
",
    },
    Row {
        row: "S7: seconds to wait for a connection",
        script: "\
! s 7=50
> ATS7=2\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ elapsed 1999
! mode handshaking
@ elapsed 1
! hangup timeout
< \\r\\nNO ANSWER\\r\\n
! mode command
> ATA\\r
! answer
@ elapsed 2000
! hangup timeout
< \\r\\nNO CARRIER\\r\\n
! mode command
",
    },
    Row {
        row: "S10: carrier loss delay; stored",
        script: "\
! s 10=14
> ATS10=20S10?\\r
< \\r\\n020\\r\\n\\r\\nOK\\r\\n
! s 10=20
",
    },
    Row {
        row: "S12: escape guard time in units of 20 ms",
        script: "\
! s 12=50
> ATS12=10\\r
< \\r\\nOK\\r\\n
> ATD5\\r
! dial 5
@ connected
< \\r\\nCONNECT\\r\\n
@ elapsed 199
> +++
! line +++
@ elapsed 200
! mode online
> +++
! line +++
@ elapsed 199
! mode online
@ elapsed 1
< \\r\\nOK\\r\\n
! mode onlinecommand
",
    },
];

fn run_rows(rows: &[Row], prelude: &str) {
    for row in rows {
        let script = row.script.replace("$ONLINE_COMMAND\n", ONLINE_COMMAND);
        common::run(row.row, &format!("{prelude}{ECHO_OFF}{script}"));
    }
}

#[test]
fn every_row_of_the_command_table() {
    run_rows(COMMANDS, "");
    run_rows(UNKNOWN_IS_ERROR, "@ config unknown_command=error\n");
    // Every command in the handoff's table has at least one row here.
    for command in [
        "A",
        "D",
        "E0",
        "H",
        "I",
        "L",
        "O",
        "Q0",
        "S<n>",
        "V0",
        "X0",
        "X4",
        "Z",
        "&F",
        "&C0",
        "&D0",
        "&D1",
        "&D2",
        "&D3",
        "&K0",
        "&V",
        "&W",
        "+MS",
        "anything else",
    ] {
        assert!(
            COMMANDS.iter().any(|row| row.row.starts_with(command)),
            "no row for {command}"
        );
    }
}

#[test]
fn every_row_of_the_register_table() {
    run_rows(REGISTERS, "");
    for register in ["S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7", "S10", "S12"] {
        let prefix = format!("{register}:");
        assert!(
            REGISTERS.iter().any(|row| row.row.starts_with(&prefix)),
            "no row for {register}"
        );
    }
}

#[test]
fn the_initial_s0_comes_from_the_line_configuration() {
    common::run(
        "configured S0",
        "\
@ config auto_answer_rings=1
! s 0=1
> ATS0=5\\r
< ATS0=5\\r\\r\\nOK\\r\\n
> AT&F\\r
< AT&F\\r\\r\\nOK\\r\\n
! s 0=1
@ ring
< \\r\\nRING\\r\\n
! answer
",
    );
}

#[test]
fn a_command_line_of_255_characters_is_accepted_and_256_is_not() {
    // Clause 5.2.1 leaves the limit to the DCE; the handoff sets it at 255
    // characters after the prefix.
    let long = "E0".repeat(127); // 254 characters
    common::run(
        "255 characters",
        &format!("{ECHO_OFF}> AT{long}E\\r\n< \\r\\nOK\\r\\n\n"),
    );
    common::run(
        "256 characters",
        &format!("{ECHO_OFF}> AT{long}E0\\r\n< \\r\\nERROR\\r\\n\n> A/\n< \\r\\nOK\\r\\n\n"),
    );
}

#[test]
fn result_code_numbers_are_the_handoffs() {
    use at::ResultCode::*;
    let table = [
        (Ok, 0, "OK"),
        (Connect, 1, "CONNECT"),
        (Ring, 2, "RING"),
        (NoCarrier, 3, "NO CARRIER"),
        (Error, 4, "ERROR"),
        (NoDialtone, 6, "NO DIALTONE"),
        (Busy, 7, "BUSY"),
        (NoAnswer, 8, "NO ANSWER"),
    ];
    for (code, number, text) in table {
        assert_eq!(code.number(), number);
        assert_eq!(code.text(), text);
    }
    assert_eq!(at::connect_number(0), 1);
    assert_eq!(at::connect_number(300), 1);
    assert_eq!(at::connect_number(1200), 5);
    assert_eq!(at::connect_number(2400), 10);
    assert_eq!(at::connect_number(4800), 11);
    assert_eq!(at::connect_number(9600), 12);
    assert_eq!(at::connect_number(14400), 13);
    assert_eq!(at::connect_number(19200), 14);
    assert_eq!(at::connect_number(33600), 1);
}
