# Signing

RustModem has a driver, and Windows installs a driver only from a package
whose signature it trusts. This document says how a build is signed in the
lab today, what a machine that installs such a build is trusting, and what
it would take to sign a build that anybody can install.

**Read "What is established and what is not" before relying on any of
it.** One thing in particular has not been tested: installing a lab build
on a machine with Secure Boot on and test-signing off. Every machine it
has been installed on is a CI runner, which has Secure Boot off and
test-signing on.

## What gets signed

| File | Signed by | Why |
|---|---|---|
| `rmvcom.cat`, the catalog of the driver package | `signtool`, in `driver\package.ps1` | It is what Plug and Play checks. The catalog has the hashes of `rmvcom.dll`, `rmvcom.inf` and `rmmodem.inf`, so signing it signs all three |
| `rmvcom.dll`, the driver | The same | So that the file says who made it by itself |
| `rustmodem-svc.exe`, `rustmodem-manager.exe`, `rustmodem-ctl.exe` | `signtool`, in `ci\build.ps1` | Windows shows the publisher when it asks to run them as an administrator |
| `rustmodem-setup.exe` | The same, last | It is the file a person downloads. The signature goes after the files it carries, and it finds them all the same |

The driver is a user-mode driver. It is loaded by Windows' driver host
process, not by the kernel, and nothing in RustModem is loaded by the
kernel.

## The lab flow

On the machine that builds. It needs a Windows Driver Kit and LLVM;
`docs/DRIVER.md` says how to get them.

1. **Make a lab certificate, once.**

   ```
   ci\lab-certificate.ps1 -Out C:\keys\rustmodem-lab.cer
   ```

   It makes a self-signed code-signing certificate named `RustModem Lab`
   in the certificate store of the account that runs it, writes the
   certificate without its key to the file, and prints the thumbprint.
   Keep the file outside the repository. **The private key stays in that
   account's certificate store** and is not exported by anything here; to
   build on another machine, make another certificate there.

2. **Build.**

   ```
   ci\build.ps1 -Thumbprint <the thumbprint> -LabCertificate C:\keys\rustmodem-lab.cer
   ```

   The result is `target\setup\rustmodem-setup.exe`, a lab build: it
   carries the certificate, and because it does, it asks to trust it.

3. **Install**, on the machine RustModem is to run on.

   ```
   rustmodem-setup.exe
   ```

   Setup says that it is a lab build and what trusting the certificate
   means, and asks. Silently, the answer has to be given beforehand:

   ```
   rustmodem-setup.exe /quiet --trust-lab-certificate
   ```

   A silent install that is not told so installs nothing and ends with
   exit code 4 and `RMD9509`.

No private key is committed, logged or put in a setup program. The build
script never reads the key: it gives `signtool` a thumbprint and the
certificate store does the signing. CI makes a certificate for each run on
a machine that is destroyed after it, and that key is gone with the
machine.

## What a machine that trusts the lab certificate is trusting

Setup puts the certificate in two of the machine's stores:

| Store | What it means |
|---|---|
| Trusted Root Certification Authorities | The machine accepts the certificate as its own authority: a signature made with its key verifies |
| Trusted Publishers | Windows installs a driver package signed with its key without asking who the publisher is |

Both are needed. Without the first the signature does not verify; without
the second Windows asks, on the desktop, whether to trust the publisher,
and an install that nobody is watching waits on that question for ever.
The spike found this by having a job hang on it: setup therefore trusts
the certificate before it touches the driver store, and never the other
way round.

**The consequence is that whoever holds the certificate's private key can
sign anything, and that machine will take it as trusted**: a driver
package, a program, and, because the certificate is a root on that
machine, whatever else Windows checks against its roots that a
code-signing certificate can vouch for. That is why:

- a lab build says so and asks, and a silent one must be told;
- the key belongs on the build machine and nowhere else;
- Microsoft's own documentation calls putting a certificate in Trusted
  Publishers something "only suggested for your internal systems" that
  "should never be followed for any driver package that is distributed
  outside your organization";
- uninstalling RustModem takes the certificate out of both stores.

A lab build is for machines you test on, and for a machine you run
yourself and have decided to trust your own key on. It is not something
to give to anyone else.

## The path to distribution

To be installed by people who have not chosen to trust a key of ours, the
driver package has to be signed by something their machine trusts
already. Microsoft's documentation, as read in November 2026, gives these
ways. The pages are cited with the date each says it was last updated.

| Way | What it is | What Microsoft says of it |
|---|---|---|
| **Certification through the Hardware Lab Kit** (WHCP) | The driver is run through Microsoft's tests in the kit and the results are submitted through the hardware dashboard; Microsoft signs the package | "The recommended method"; it "signs a driver for all OS versions" [1] |
| **Attestation signing** | The package is submitted through the hardware dashboard with no test results; Microsoft signs it | "For testing purposes only"; it "can't be published to Windows Update for retail audiences"; it "only works on Windows 10 Desktop and later"; and "Windows Server 2016 and greater doesn't accept attested device and filter driver signing submissions" [1] [2]. Not a way to a Windows Server, which is where RRAS runs |
| **A certificate from a public authority, and no signature of Microsoft's** | The catalog is signed with an ordinary code-signing certificate | For a user-mode driver on 64-bit Windows the rule for Plug and Play is that the catalog "must be signed by WHQL or signed by a third-party release certificate" [3]. The rule that a driver must be signed by Microsoft is about kernel-mode drivers [4]. The person installing is asked once whether to trust the publisher, unless the publisher is trusted already [5] |

Both of the first two need an account in the Hardware Developer Program,
and to register for one: "You must have an extended validation (EV) code
signing certificate... You only need the certificate to register... You
don't need to sign your driver with it." [6]

### What each would cost

`NEEDS-CODY`: these are decisions, and the figures are for making them,
not quotations.

| | Money | Work |
|---|---|---|
| Lab certificate only | None | None beyond what is built. RustModem can then be installed only on machines whose owner trusts our key |
| A certificate from a public authority | An organisation-validated or extended-validation code-signing certificate. Resellers' prices seen in November 2026 were roughly 300 to 700 US dollars a year, and certificates are now issued for at most 460 days. The key has to be kept on a hardware token or in a cloud signing service, which the authorities require | `ci\build.ps1` already signs with whatever certificate it is given a thumbprint of, and takes a timestamp server. The release workflow would need the signing service's credentials as secrets. Whether Windows Server installs a user-mode driver signed this way without a signature of Microsoft's is stated by [3] and has not been tried |
| Attestation | An extended-validation certificate, to register | A dashboard submission for each release. Does not reach Windows Server |
| Certification | An extended-validation certificate, to register. The kit itself costs nothing | A test machine and a controller set up as the kit asks, the tests run on each Windows version to be supported, and a submission for each release. A virtual COM port and a modem INF have device classes with tests of their own, and what those ask of a port with no hardware is not known here |

The certificate from a public authority is the smallest step that makes a
build other people can install, and it is the one to try first: make a
build with it, and install it on a clean Windows 11 and a clean Windows
Server with Secure Boot on. If Windows installs the driver and asks only
whether to trust the publisher, that is a distribution build. If it does
not, certification is what is left.

## What is established and what is not

**Established**, by doing it on GitHub-hosted runners (Windows Server
2022 and 2025; `docs/DRIVER_SPIKE.md`):

- A catalog signed with a self-signed certificate that is in the
  machine's Trusted Root and Trusted Publishers stores verifies, and
  `pnputil` adds the package without asking.
- With the certificate not trusted, `pnputil` does not fail: it waits on
  a question nobody sees.
- The driver then installs on a device and loads, and the whole of
  `ci\install-test.ps1` passes, on every push.

**Not established:**

- **Secure Boot on and test-signing off.** Every runner has Secure Boot
  off and test-signing on, and a runner cannot be restarted to change
  either. Microsoft's documentation says a user-mode driver needs no
  signature of Microsoft's and that Plug and Play installs a package
  whose publisher is trusted [3] [5], and it says the rule Secure Boot
  enforces is for kernel-mode code [4]. No page of Microsoft's and no
  first-hand report was found that says what happens to such a package
  with Secure Boot on, either way. So it is expected to work, and it is a
  belief. `NEEDS-CODY`: criteria 8 and 9 of `docs/PHASE5.md`. Cody has
  said that if it does need Secure Boot off, that is accepted.
- **Windows 11.** Nothing here has been installed on a client edition.
- **Smart App Control and App Control for Business.** Either can refuse a
  DLL signed by a certificate it does not know [7]. Neither is on by
  default on a server; Smart App Control can be on a new Windows 11.
- **Every row of "The path to distribution" but the first sentence of
  each.** They are what the documentation says. None has been done.

## Sources

Read on learn.microsoft.com in November 2026.

1. "Driver signing offerings", updated 2026-04-14.
   `windows-hardware/drivers/dashboard/driver-signing-offerings`
2. "Attestation sign Windows drivers", updated 2026-04-14.
   `windows-hardware/drivers/dashboard/code-signing-attestation`
3. "PnP device installation signing requirements", updated 2025-07-18.
   `windows-hardware/drivers/install/pnp-device-installation-signing-requirements--windows-vista-and-later-`
4. "Kernel-mode code signing policy", updated 2025-07-18.
   `windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later-`
5. "Trusted Publishers certificate store", updated 2025-07-18.
   `windows-hardware/drivers/install/trusted-publishers-certificate-store`
6. "Register for the Microsoft Windows Hardware Developer Program",
   updated 2026-08-13.
   `windows-hardware/drivers/dashboard/hardware-program-register`
7. "App Control for Business", updated 2026-11-17.
   `windows/security/application-security/application-control/app-control-for-business/appcontrol`
