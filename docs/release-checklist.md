# Release checklist

The manual gate for every beta and for 1.0 ([#46](https://github.com/theHimanshuShekhar/bhayanakshare/issues/46), "Betas and the gate"). The automated tests cover the core, the shell's decisions and the UI. This page covers what only two real PCs, real firewalls and real installers can show.

**Copy this page into the release ticket** ([#60](https://github.com/theHimanshuShekhar/bhayanakshare/issues/60), [#61](https://github.com/theHimanshuShekhar/bhayanakshare/issues/61), [#71](https://github.com/theHimanshuShekhar/bhayanakshare/issues/71)) **and fill the copy in there.** This file stays blank.

- **Result**: Pass, Fail, or N/A. N/A is allowed only on a row that says "if", and on the update rows of the first beta, which has no earlier release to update from. A Fail gets an issue, linked in Notes.
- **Build**: the version and the file tested, such as `0.9.1, x64-setup.exe` or `0.9.1, AppImage`.
- **Notes**: anything seen, and any figure a row asks for.
- **A ticket number under an ID** (such as [#54](https://github.com/theHimanshuShekhar/bhayanakshare/issues/54)) means the row can only pass once that ticket has merged. Until then it fails by design; once it has, the row is part of the gate like any other.

A beta or 1.0 passes the gate when every row is Pass or a justified N/A. 1.0 runs the whole list once more on its final build, after both betas have passed. The update rows (UPD) can only run once a release is published, since the updater reads the latest published release: run them straight after publishing. An update row that fails then is fixed forward in a patch release (for 1.0, in 1.0.1), and the failing release is not left as the latest.

The words in quotes are the app's own (`ui/src/i18n.ts`). If the wording has moved on, the row's meaning wins.

| Run | |
|---|---|
| Release and ticket | |
| Tester and date | |
| Win: Windows 11 build (`winver`), account, antivirus | |
| Lin: distro and version, desktop session (Wayland or X11) | |
| Lin's firewall (firewalld zone, or ufw) | |
| LAN: wired or Wi-Fi, same subnet, Wi-Fi client isolation off | |
| Files tested: installer, AppImage, deb or rpm | |

## Before the manual rows

The tests that stand in for the internet rows (see Not in the gate) do not run in CI, so run them on the release's commit first, on a Linux machine with internet access. Record each as a row:

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| AUTO-1 | `cargo test -p bhayanakshare-core --features relay-tests --test relay_limit -- --nocapture` | Passes: a Transfer completes through a rate-limited relay. | | | |
| AUTO-2 | `BHAYANAKSHARE_TEST_DHT=1 cargo test -p bhayanakshare-core --lib dht` | Passes: the Device is found in the public DHT. | | | |
| AUTO-3 | CI on the release's commit | `check` and `check-windows` both passed. | | | |

## Setting

Two PCs on one LAN, **with their firewalls on**:

- **Win**: Windows 11 x64, Windows Defender Firewall on, Defender real-time protection on. Use an **administrator** account (only an administrator can answer the firewall prompt) whose **profile folder has a space in its path**, such as `C:\Users\Test User` (WIN-7 needs that).
- **Lin**: Linux x86_64, Ubuntu 22.04+ or Fedora 39+, with the distro's firewall as it ships (firewalld or ufw). The AppImage is used for most rows. The package of the distro's own format (deb on Ubuntu, rpm on Fedora) has the rows marked "package", done last, on the same PC. The app treats any install that is not an AppImage or the Windows installer alike, so the one package is enough for the gate.

In the steps, **Win** and **Lin** are the Device Names to give the two Devices on first run. **Win2** and **Lin2** are second Devices for the Batch rows, started on the same PC with their own folders (BAT-1).

Leave both firewalls as the system made them. The rows check the prompts and the steps in [`docs/firewall.md`](firewall.md); do not allow anything in advance. The two PCs must reach each other (not a guest network; see Other causes in the firewall page).

### Test content

Make this on Lin. Win has none: it sends back what it received, so do each Lin to Win row before its Win to Lin pair.

```sh
mkdir testset && cd testset
head -c 5M /dev/urandom > one.bin                        # a file
head -c 1G /dev/urandom > big.bin                        # PERF, WIN-4: 1 GiB
head -c 16G /dev/urandom > long.bin                      # RES, UPD-4, UPD-5: see below for its size
base64 -w 76 /dev/urandom | head -c 100000 > long.txt    # TRF-5: text over 64 KiB
mkdir -p tree/a/b tree/c                                 # a folder: nested, 100 files, a link, a name Windows cannot hold
for i in $(seq 1 100); do head -c 50K /dev/urandom > "tree/a/b/f$i.bin"; done
echo hello > tree/c/note.txt && ln -s note.txt tree/c/link && touch 'tree/a:b?.txt'
mkdir small                                              # WIN-2
for i in $(seq 1 2000); do head -c 4K /dev/urandom > "small/f$i.bin"; done
# WIN-5: a tree whose paths pass 260 characters (six folders of 51 characters, about 330 in all)
d=$(printf 'd%.0s' $(seq 1 50)); p=longtree
for i in 1 2 3 4 5 6; do p="$p/$d$i"; done
mkdir -p "$p" && echo hello > "$p/file.txt"
```

`long.bin` must keep a Transfer running for 2 minutes or more, so that a PC can be put to sleep or its app killed part-way. 16 GiB does that on wired gigabit; size it from the rate PERF-1 shows on a faster or slower link. Each copy needs that much free space on the receiving PC: delete the copies between rows.

**Checking a result.** Compare a file's SHA-256 with its source, and a folder's file count and total size:

```sh
# Lin
sha256sum FILE
find DIR -type f -printf '%s\n' | awk '{ n++; s += $1 } END { print n, s }'
```

```powershell
# Win
Get-FileHash -Algorithm SHA256 -LiteralPath FILE
Get-ChildItem -LiteralPath DIR -Recurse -File | Measure-Object -Property Length -Sum
```

### Order

1. INS-1, FW-1, then INS-2 (its first start raises the firewall prompt, FW-2), INS-3, FW-5, FW-3.
2. The rest of the Linux and Windows rows. FW-4 takes Win's firewall rule away for a while and puts it back, so do it when no other Windows row is under way. The PORT rows can be done any time after FW-2.
3. The package rows (INS-4, LNK-5, UPD-3, UNI-6) after every AppImage row.
4. The update rows (UPD) once the release is **published**. The updater reads only the latest published release ([Cutting a release](../README.md#cutting-a-release)), so a draft cannot be updated to. Until then, test the draft's own files, as above. For the UPD rows put the **previous** release on each PC in place of the one under test, before publishing: Windows, uninstall without the option to delete data and run the previous installer; AppImage, run the previous file; package, remove the new one and install the previous one. The data folder stays through all three, so it is the same Device. Publish, then run the rows. The first beta has no previous release: its UPD rows are N/A.
5. The uninstall rows (UNI) last, on the build the update left.

## Install and first run

Install and update steps are in the [README](../README.md#install-and-update).

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| INS-1<br>[#57](https://github.com/theHimanshuShekhar/bhayanakshare/issues/57) | Win: download `BhayanakShare_X.Y.Z_x64-setup.exe` and run it. At SmartScreen choose More info, then Run anyway. (If Smart App Control blocks it outright, turn that off first; see [Installing on Windows](../README.md#installing-on-windows).) Do not start the app yet. | No UAC prompt and no administrator rights asked. It installs for this user only, under `%LOCALAPPDATA%`, not Program Files. A Start menu entry, and an entry in Settings, Apps, Installed apps. The installer is a `-setup.exe`, with no `.msi`. | | | |
| INS-2 | Win: start BhayanakShare from the Start menu. Answer the firewall prompt as in FW-2. On the first-run screen set the Device Name to `Win`, leave the rest as shown, and choose Get started. | The first-run screen shows Device Name (the PC's hostname), Visibility "People who have my ID", start at login on, and the save folder `Downloads\BhayanakShare`. Get started opens Home. Settings → App shows the version being tested. Settings → This Device shows My ID: write Win's Fingerprint in Notes. | | | |
| INS-3 | Lin: `chmod +x` the AppImage and run it. On the first-run screen set the Device Name to `Lin` and choose Get started. | The same defaults, with the save folder `~/Downloads/BhayanakShare`. Settings → App shows the version being tested. Write Lin's Fingerprint in Notes. | | | |
| INS-4<br>package | Lin, after the AppImage rows: delete the AppImage file. Install the package (`sudo apt install ./BhayanakShare_X.Y.Z_amd64.deb`, or `sudo dnf install ./BhayanakShare-X.Y.Z-1.x86_64.rpm`) and start the app from the application menu. | It installs without asking anything beyond the package manager's own confirmation, and starts. There is no first-run screen, as the data folder is the AppImage's: Settings → This Device shows the same Fingerprint as in INS-3, and the Contacts are still there. | | | |

## Firewalls

Steps for both systems are in [`docs/firewall.md`](firewall.md).

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| FW-1<br>[#57](https://github.com/theHimanshuShekhar/bhayanakshare/issues/57) | Win, after INS-1 and before the first start: Windows Security → Firewall & network protection → Advanced settings → Inbound Rules (or run `wf.msc`). Look for any rule for BhayanakShare. | There is none. The installer adds no firewall rule. | | | |
| FW-2 | Win: start the app for the first time (INS-2). When "Windows Defender Firewall has blocked some features of this app" appears, as the administrator tick **Private networks** only and choose **Allow access**. Check beforehand that the network's profile is Private (Settings → Network and internet). | The prompt appears once, the first time the app listens. Afterwards Inbound Rules has Allow rules for the program, for Private. | | | |
| FW-3 | Set Visibility to Everyone on both Devices (Settings → Privacy). Cut the LAN off from the internet (unplug the router's uplink). Send `one.bin` from Lin to Win, and what Win received back to Lin. Reconnect the internet. | Both appear Nearby on each other and both Transfers complete with the internet cut, so they went over the LAN. (Without a rule for the program they would take the relay, or fail offline.) | | | |
| FW-4 | Win: delete the app's Allow rules in Inbound Rules, quit the app (tray → Quit) and start it again. The prompt returns: choose **Cancel**. Wait 30 seconds on Home. Then follow "To undo a block" in [Windows](firewall.md#windows) exactly, step by step. | Windows creates Block rules for the program (usually two, TCP and UDP) and does not ask again. Lin is not Nearby on Win, nor Win on Lin, and after 30 seconds Home shows "No Devices found on this network yet…" with "How to allow local discovery". After the undo steps the prompt comes back, and Allow on Private restores Nearby both ways. Every step of the page worked as written; write in Notes any that did not. | | | |
| FW-5 | Lin: with the firewall on, do only what [Linux](firewall.md#linux) says for this distro (Fedora's firewalld: the `home` zone, or the `mdns` service in the zone; Ubuntu's ufw normally already accepts mDNS, so add the commands only if Nearby stays empty). | Win appears Nearby on Lin, Lin on Win, and a Transfer between them completes. Every command of the page worked as written. Write the firewall and zone in the run table. | | | |
| FW-6 | Win, **if** Bonjour is installed (`Get-Service 'Bonjour Service'` shows Running; it comes with iTunes and some printer software): restart the app. Do not install Bonjour for this row. | Bonjour shares UDP port 5353 with the app, so Home shows no "could not start" hint, and Lin appears Nearby on Win and Win on Lin. If the hint shows, Bonjour holds the port without sharing: Fail, and say so in Notes. | | | |

## UDP port 5353 held

[#49](https://github.com/theHimanshuShekhar/bhayanakshare/issues/49) added a hint on Home when local discovery cannot start. Windows CI cannot test a port that is held without sharing, because the hosted runner already shares UDP 5353, so this is checked by hand ([When Home says local discovery could not start](firewall.md#when-home-says-local-discovery-could-not-start)). Win's Visibility is Everyone at the start; Lin is Nearby on it.

**Holding the port.** Windows lets programs share a UDP port unless one asks for exclusive use. The app shares it, like Windows' own mDNS and browsers, so the holder must bind with `SO_EXCLUSIVEADDRUSE` (`ExclusiveAddressUse` in .NET), set before it binds, on IPv4 and on IPv6 (Home reports a problem only when local discovery cannot run at all, and it runs on IPv6 if only IPv4 is held). Paste the script below into a Windows PowerShell window (the one Windows 11 has), or save it as `hold-5353.ps1` and run `powershell -ExecutionPolicy Bypass -File hold-5353.ps1` (Windows' default policy refuses scripts). It holds the port until Enter is pressed; closing the window releases it too. If it prints "IPv6 not held", the app may still run local discovery on IPv6 and show no hint at PORT-1: write the message in Notes.

```powershell
$ErrorActionPreference = 'Stop'
$held = @()

$v4 = [System.Net.Sockets.Socket]::new(
    [System.Net.Sockets.AddressFamily]::InterNetwork,
    [System.Net.Sockets.SocketType]::Dgram,
    [System.Net.Sockets.ProtocolType]::Udp)
$v4.ExclusiveAddressUse = $true
$v4.Bind([System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 5353))
$held += $v4

try {
    $v6 = [System.Net.Sockets.Socket]::new(
        [System.Net.Sockets.AddressFamily]::InterNetworkV6,
        [System.Net.Sockets.SocketType]::Dgram,
        [System.Net.Sockets.ProtocolType]::Udp)
    $v6.DualMode = $false
    $v6.ExclusiveAddressUse = $true
    $v6.Bind([System.Net.IPEndPoint]::new([System.Net.IPAddress]::IPv6Any, 5353))
    $held += $v6
} catch {
    Write-Host "IPv6 not held: $($_.Exception.Message)"
}

Read-Host 'Holding UDP 5353. Press Enter to release' | Out-Null
foreach ($s in $held) { $s.Close() }
```

If the first bind fails with "Only one usage of each socket address…", another program already has the port, and an exclusive bind cannot join it. Find it with `Get-NetUDPEndpoint -LocalPort 5353` (the `OwningProcess` is its process ID; `Get-Process -Id <that number>` names it). Close that program (a browser, or stop Bonjour with `Stop-Service 'Bonjour Service'` as an administrator) and run the script again. If the owner is Windows' own DNS Client (the `svchost` hosting the `Dnscache` service, which cannot be stopped), turn its mDNS off for the test: as an administrator, `reg add "HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters" /v EnableMDNS /t REG_DWORD /d 0 /f`, then restart Windows. Run the PORT rows, then `reg delete "HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters" /v EnableMDNS /f` and restart again. That value is documented by third-party guides, not by this project, and has not been tried here; say in Notes what held the port and what was done about it.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| PORT-1<br>[#49](https://github.com/theHimanshuShekhar/bhayanakshare/issues/49) | Win: quit the app (tray → Quit). Run the holder and wait for its "Holding UDP 5353" prompt. Start the app. Visibility is Everyone. Look at Home. | Home shows at once that local discovery could not start ("Local discovery could not start, so Nearby Devices can't be found.") with the reason "Another program may be using the port it needs (UDP 5353), or security software blocked it. BhayanakShare keeps trying." and the link "How to allow local discovery". It takes the place of the 30-second firewall hint. Lin is not Nearby on Win. | | | |
| PORT-2<br>[#49](https://github.com/theHimanshuShekhar/bhayanakshare/issues/49) | Release the port (Enter in the holder's window). Do not restart the app. Time how long the hint stays. | The hint goes away by itself within about 15 seconds. Lin then appears Nearby on Win, and Win on Lin. | | | |
| PORT-3<br>[#49](https://github.com/theHimanshuShekhar/bhayanakshare/issues/49) | Run the holder again. On Win set Visibility to Hidden (Settings → Privacy). | Home shows the hint for a Hidden Device: "Local discovery could not start, so people who have your ID can't find this Device on the network." with the same reason about UDP 5353, and the link "How to allow local discovery". It takes the place of "You're Hidden, so Nearby Devices aren't shown.", which is not shown while the port is held. | | | |
| PORT-4<br>[#49](https://github.com/theHimanshuShekhar/bhayanakshare/issues/49) | Release the port. Do not restart the app. Time how long the hint stays. Then set Visibility back to Everyone. | The hint goes away by itself within about 15 seconds, and "You're Hidden…" shows in its place while Win is Hidden. With Everyone again, Win and Lin are Nearby on each other. | | | |

## Discovery under each Visibility

Visibility is set in Settings → Privacy. The Device whose Visibility is set is the subject (S); the other is the observer (O). Neither Device has the other as a Contact at the start of DIS-1. A Nearby Device should show within the 30 seconds after which Home would suggest the firewall page. See [Visibility in CONTEXT.md](../CONTEXT.md).

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| DIS-1 | S = Win, Visibility **Everyone**. O = Lin, "People who have my ID", with no Contact for Win. | Win is under Nearby on Lin, named `Win`, with a Fingerprint equal to Win's My ID. It is not a Contact (the tile offers "Save as Contact…"). | | | |
| DIS-2 | S = Lin, **Everyone**. O = Win, "People who have my ID", with no Contact for Lin. | Lin is under Nearby on Win, named `Lin`, with Lin's Fingerprint. | | | |
| DIS-3 | S = Win, **People who have my ID**. O = Lin with no Contact for Win. Wait 60 seconds. Then on Lin: Contacts → Add Contact…, paste Win's Device ID, check the Fingerprint, "It matches, add Contact". | Win is **not** under Nearby on Lin for the 60 seconds. Within 30 seconds of adding the Contact, Win appears Nearby on Lin, as a Contact. | | | |
| DIS-4 | The same, S = Lin, O = Win (Win has no Contact for Lin until the end). | Lin is **not** Nearby on Win until Win adds Lin as a Contact; then it appears. | | | |
| DIS-5 | S = Win, **Hidden**. O = Lin, **Everyone**, with Win as a Contact. | Win is not under Nearby on Lin, though Lin holds its Device ID; its Contact tile has no Nearby mark. Win's Home says "You're Hidden, so Nearby Devices aren't shown." and lists no Nearby Device, though Lin is on Everyone. | | | |
| DIS-6 | S = Lin, **Hidden**. O = Win, **Everyone**, with Lin as a Contact. | The same, the other way round. | | | |
| DIS-7 | With both on Everyone and Nearby to each other: on Win change the Device Name (Settings → This Device) and then switch Visibility Everyone → Hidden → People who have my ID. Do not restart. Then the same on Lin. | Each change reaches the other Device within about a minute, with no restart: the new name shows; the Device leaves Nearby when Hidden and is back on "People who have my ID" (the two are Contacts by now, so each holds the other's ID). | | | |

## Files, folders and text

Accept is exercised here and in every Transfer below. Both Devices are on Everyone and Nearby to each other. Choose the tile ("Send to Lin"), then Choose files…, Choose folder… or Write text….

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| TRF-1 | Lin → Win: send `one.bin`. Accept on Win. | The Offer sheet shows the Sender with its Fingerprint, the file and its size, the save folder and an "Expires in" countdown. After Accept both rows go to Received and Sent. The file is in Win's save folder with the same SHA-256 as the source; "Show in folder" opens Explorer on it. Both Devices' History list the Transfer. | | | |
| TRF-2 | Win → Lin: send the `one.bin` Win received. Accept on Lin. | The same, the other way. `one.bin` has the same SHA-256 on Lin as at the start. "Show in folder" opens the file manager. | | | |
| TRF-3 | Lin → Win: send the folder `tree`. Accept on Win. | The Offer sheet says "1 name adjusted", and Lin's row says "1 link skipped". The folder arrives with the same structure; its file count and total size equal the source's (`find -type f` does not count the link); the file `a:b?.txt` is saved as `a_b_.txt`. | | | |
| TRF-4 | Win → Lin: send the `tree` Win received. Accept on Lin. | The folder arrives whole, with the same file count and total size as on Win. | | | |
| TRF-5 | Lin → Win: Write text…, send a short text with a line break and a non-ASCII letter. Then send text of more than 64 KiB (open `long.txt` in a text editor, select all, copy, paste). Accept each on Win. | The short text shows on Win as text, and "Copy" puts exactly it on the clipboard ("Copied"). The long text arrives as a file called `text.txt` in the save folder, with the same SHA-256 as `long.txt`. | | | |
| TRF-6 | Win → Lin: the same, both texts (for the long one, open the `text.txt` Win received, select all, copy, paste). | The same on Lin. | | | |

## Batch

A Batch needs two Receivers, and there are two PCs, so each PC runs a second Device. Start it from a terminal on that PC, with its own folders; it has its own key and does not start at login ([Running two instances](../README.md#running-two-instances-on-one-machine)). On its first-run screen give it its name and set Visibility to Everyone.

- Lin2: `BHAYANAKSHARE_DATA_DIR=/tmp/bhs-2/data BHAYANAKSHARE_SAVE_DIR=/tmp/bhs-2/save ./BhayanakShare_X.Y.Z_amd64.AppImage`
- Win2, in PowerShell, starting the installed program (the Target of its Start menu shortcut): `$env:BHAYANAKSHARE_DATA_DIR = "$env:TEMP\bhs-2\data"; $env:BHAYANAKSHARE_SAVE_DIR = "$env:TEMP\bhs-2\save"; & '<the program>'`

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| BAT-1 | Start Lin2. On Win select the tiles of Lin and Lin2 and send the folder `tree`. Accept on Lin; **Decline** on Lin2. | Win shows one row, "tree to 2 Devices". Lin and Lin2 each get their own Offer, and neither shows the other. The Batch reads "1 of 2 delivered, 1 declined"; "Show each Device" lists each Receiver's state; History has one entry for the Batch. Lin has the folder whole; Lin2's save folder has nothing. | | | |
| BAT-2 | Quit Lin2. Start Win2. On Lin select the tiles of Win and Win2 and send the folder `tree`. Accept on Win; **Decline** on Win2. | The same, the other way. Quit Win2 afterwards. | | | |

## Accept, decline and expiry

Accept is TRF-1 to TRF-6.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| ANS-1 | Lin → Win: send `one.bin`. **Decline** on Win. | Lin's row says "Win declined."; Win's says "You declined." Nothing is saved. Both Histories record it. | | | |
| ANS-2 | Win → Lin: the same, declined on Lin. | The same, the other way. | | | |
| ANS-3 | Lin → Win and Win → Lin at the same time: send `one.bin` each way, and answer neither Offer for 10 minutes. Leave each window open. | The "Expires in" countdown runs down on both. At zero Lin's row says "Win did not answer in time. The Offer expired." and Win's row says the same naming Lin, for the Offers each sent; each Receiver's row says "The Offer expired before you answered." and Accept is gone. Nothing is saved. Both ends expire within a few seconds of each other. | | | |
| ANS-4 | After ANS-3: send `one.bin` again each way and Accept at once. | Both Transfers complete: an expired Offer leaves nothing behind that stops the next. | | | |

## Resume

Resume is driven by the Receiver. Interrupt only once the row shows Receiving with a percentage, so the Receiver holds the content hash (before that a Transfer fails instead of resuming), and not in the first seconds of it (a kill in the first second of a fetch loses that second's data, [section 4 of the spec](spec/v1.md)). `long.bin` must keep the Transfer running for 2 minutes or more (see Test content). Check each result with the SHA-256 against the source.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| RES-1 | Lin → Win: send `long.bin`, Accept. About a third of the way, put **Win** to sleep (Start, Power, Sleep). Wake it after 2 minutes. | Win's row shows "Lost contact with Lin. Reconnecting…" and then Receiving again, without starting over from 0%, and ends Received. The SHA-256 matches. Lin's row ends Sent. The Transfer is listed once. | | | |
| RES-2 | Win → Lin: send the `long.bin` Win now holds. About a third of the way, suspend **Lin** (`systemctl suspend`). Wake it after 2 minutes. | The same, on Lin. | | | |
| RES-3 | Win → Lin: send `long.bin`, Accept. About a third of the way, put **Win**, the Sender this time, to sleep. Wake it after 2 minutes. | Lin's row shows "Lost contact with Win. Reconnecting…" while Win sleeps and, once Win is awake, goes on to Received. The SHA-256 matches. | | | |
| RES-4 | Lin → Win: send `long.bin`, Accept. About a third of the way, kill Win's app: Task Manager → Details → End task on the BhayanakShare process (closing the window only hides it). Start the app again from the Start menu. | The Transfer is listed again and resumes by itself ("Reconnecting…", then Receiving), and ends Received with the SHA-256 matching. Until then nothing of it is in the save folder except the hidden `.bhayanakshare-incoming` folder. The Device ID is unchanged. | | | |
| RES-5 | Win → Lin: send `long.bin`, Accept. About a third of the way, `kill -9` Lin's app (`pgrep -a bhayanakshare` shows it; Lin2 must be quit). Start it again. | The same, on Lin. | | | |
| RES-6 | Lin → Win: send `long.bin`, Accept. About a third of the way, `kill -9` Lin's app, the **Sender** this time. Start it again. | Win's row shows "Lost contact with Lin. Reconnecting…". Once Lin is back the Transfer goes on to Received, with the SHA-256 matching. Lin's row is the same Transfer, not a new one. | | | |

## Sending to a Hidden Device by its Device ID

Switch off the LAN's internet connection for these rows, so that only the local network can carry the Transfer, and switch it on again afterwards. The sending Device is on Everyone and does **not** have the Hidden one as a Contact (remove it first: Contacts → Remove…). Copy the Device ID from the Hidden Device (Settings → This Device → My ID → Copy ID). Afterwards put Visibility back, add the Contacts again and reconnect the internet.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| HID-1 | Lin **Hidden**. On Win choose "Send to ID…", paste Lin's Device ID, choose `one.bin`. Accept on Lin. | Lin is not listed Nearby on Win. The Offer still reaches Lin, and the file arrives with the same SHA-256. | | | |
| HID-2 | Win **Hidden**. On Lin the same with Win's Device ID. | The same, the other way. | | | |

## Share links and deep links

A Share link is `bhayanakshare://add/<Device ID>?name=<Device Name>`, shown in Settings → This Device → My ID with its QR code. Opening one anywhere should open Add Contact in the running app. The link used must be one for a Device that is **not** a Contact (remove it first, or use Lin2's or Win2's).

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| LNK-1 | Lin: Settings → This Device → My ID, which shows the Share link and its QR code. Copy link. Carry the text to Win (any chat or note). On Win: Contacts → Add Contact…, paste it. | The link has the form above. The Device ID field accepts it. The name is filled with `Lin`, as a suggestion. After Next, the Fingerprint shown equals Lin's; "It matches, add Contact" adds it. | | | |
| LNK-2 | The same from Win's link into Lin. | The same, the other way. | | | |
| LNK-3 | Win, installed: `reg query HKCU\Software\Classes\bhayanakshare\shell\open\command` (the installer registers the scheme for the user). Then, with the app running (its window may be hidden), open a Share link from outside the app: type it in Win+R, and click it in a browser or a note app (the browser may ask first to open BhayanakShare). Then quit the app (tray → Quit) and open the link again. | Running: the existing window comes forward and Add Contact opens with the Device ID and name filled. There is no second window and no second process. Not running: the app starts and shows Add Contact for that link. The `reg query` shows the installed `bhayanakshare.exe` in quotes, then `"%1"`. | | | |
| LNK-4 | Lin, AppImage, running: `xdg-open 'bhayanakshare://add/<Device ID>?name=Test'`, and click a link in a browser or chat. Quit the app and do it again. Then move the AppImage to another folder, start it from there once, quit it, and open a link again. | The same as LNK-3 in each case, including after the AppImage was moved (the app registers the scheme at every start). | | | |
| LNK-5<br>package | Lin, package installed: the same as the first part of LNK-4. | The same. The scheme is handled by the package's desktop file. | | | |
| LNK-6 | **If** a PC has a camera: on that PC, Add Contact → "Scan QR code…", and hold the other PC's My ID QR code in front of it. (Win's camera also tests WebView2's camera permission.) | The camera opens (or the system asks for permission first), the code is read, and Add Contact shows that Device. If the camera is refused, the message says how to allow it or to paste instead. Write the PC in Notes. | | | |

## Updating from the previous release

Run after the release is published, from the previous release installed on each PC (see Order). Settings → App → "Check for updates" asks at once; the app also asks when it starts. Each row ends by checking that the Device is the same one: the same Fingerprint, and its Contacts and History kept. N/A on the first beta.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| UPD-1 | Win, installed from the previous release: start the app. When "Update available (version X)" shows, press "Install and restart". | The notice offers "Install and restart", not only a link. Nothing installs before the press. It shows "Installing version X…", then "Saving progress…", then the installer's own small window with a progress bar and no questions, and no UAC prompt. The app starts again by itself, once (no second window, nothing about another instance), at the new version (Settings → App). The log (Settings → Diagnostics → Export) has "installing version X" from the old run. | | | |
| UPD-2 | Lin, previous AppImage: start it and press "Install and restart" on the notice. | The same. The AppImage file is replaced in place and the app comes back at the new version. | | | |
| UPD-3<br>package | Lin, previous deb or rpm: start the app. | The notice says "Update available (version X)" and has "Open the release page", not "Install and restart". The app installs nothing. The link opens the release page. Installing the new package by hand (`apt install ./…` or `dnf install ./…`) and starting the app gives the new version and the same Device. | | | |
| UPD-4 | Win, previous release: start a Transfer of `long.bin` from Lin and wait until it is Receiving. Then press "Install and restart". Choose "Not now". Press it again and choose "Install version X and restart". | A question first: "A Transfer is in progress. It stops for the restart and resumes when BhayanakShare is back." "Not now" leaves the Transfer running. Confirming shows "Saving progress…" until the Transfer's progress is saved (at most 30 seconds), and only then does the installer start; the app starts again at the new version by itself, and the Transfer resumes and ends Received with the SHA-256 matching. | | | |
| UPD-5 | Lin, previous AppImage: the same as UPD-4, with Win sending `long.bin`. | The same. | | | |

## Uninstall

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| UNI-1<br>[#57](https://github.com/theHimanshuShekhar/bhayanakshare/issues/57) | Win: quit the app (tray → Quit). Settings → Apps → Installed apps → BhayanakShare → Uninstall. Leave the option to delete the application data unticked. | No UAC prompt. The app is gone from the Start menu, from Installed apps and from its install folder. Kept: the data folder `%APPDATA%\dev.bhayanakshare.share`, the entry `device-secret-key.bhayanakshare` under Windows Credentials in Credential Manager, and the save folder with everything received. The firewall rules are as they were. | | | |
| UNI-2 | Win: install the same version again (INS-1) and start it. | No first-run screen. The Device Name, the Fingerprint (compare INS-2), the Contacts and the History are as before. | | | |
| UNI-3 | Win: uninstall again, this time ticking the option to delete the application data. Check that the Credential Manager entry is still there, then remove it as the [README](../README.md#installing-on-windows) says. Install and start again. | `%APPDATA%\dev.bhayanakshare.share` is gone after the uninstall, and the Credential Manager entry was still there until removed (the option does not remove the key). The new start shows the first-run screen and a new Device ID: the Fingerprint differs from INS-2. | | | |
| UNI-4 | Win, after UNI-1 or UNI-3: look under Settings → Apps → Startup, and sign out and in. | No BhayanakShare entry is left under Startup apps, and sign-in shows no error. The uninstaller is meant to delete the `BhayanakShare` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (the README says so): check with `reg query` that it is gone. | | | |
| UNI-5 | Lin, AppImage: quit the app (tray → Quit) and delete the AppImage file. Run it again from a copy. | The app runs from the copy, with the same Device ID: the data folder (under `~/.local/share`) and the key are kept. Write down anything else left behind, such as a desktop entry for the scheme in `~/.local/share/applications`, if the app wrote one (it registers the scheme at every start; the README does not say what is left). | | | |
| UNI-6<br>package | Lin, package: remove it (`sudo apt remove …` or `sudo dnf remove …`; `apt list --installed` or `rpm -qa`, filtered for "bhayanak", gives the name). | It removes without errors and runs no script that touches the firewall. The menu entry is gone, and `bhayanakshare://` links no longer open it. The data folder is kept. Write down whether a start at login entry is left in `~/.config/autostart`. | | | |

## Windows checks

These exist only on Windows, or only matter there.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| WIN-1 | The UI in **WebView2**: on the installed app go through the first-run screen (look at it at INS-2), Home, History, Contacts and every section of Settings; open the Offer sheet, Add Contact and Send to ID…; use the system's light and dark themes; narrow the window to about 640 px; set display scaling to 150% and 200%. Write the Microsoft Edge WebView2 Runtime version (Settings → Apps → Installed apps) in Notes. | Every screen draws completely, readable, with nothing cut off or unstyled. Typing, pasting, the file and folder pickers, "Copy" and "Show in folder" (Explorer opens) work. | | | |
| WIN-2 | Final move with **antivirus active**: confirm Defender real-time protection is on (Windows Security → Virus & threat protection → Manage settings) and that no exclusion covers the save folder. Lin sends the folder `small` (2,000 files). Accept on Win. | Received, with all 2,000 files, matching count and total size. No error at Saving. Defender took no action on them (Protection history). Write the time spent in Saving in Notes, if more than a minute. | | | |
| WIN-3 | The same protection: Lin sends files that Defender looks at hard: the installer `…_x64-setup.exe`, a `.zip` of `tree`, and a `.ps1` and a `.js` file with harmless text. Accept on Win. | Received, every item present with the right SHA-256, none quarantined or locked: each opens and copies at once after "Received.". | | | |
| WIN-4<br>[#54](https://github.com/theHimanshuShekhar/bhayanakshare/issues/54) | Win: make a small volume (Disk Management → Action → Create VHD, 200 MB, then initialise and format it NTFS; or a small USB stick). Set the save folder to a folder on it (Settings → Receiving). Turn Auto-accept on for Lin (Contacts). Lin sends `big.bin` (1 GiB). Then Lin sends a 1 MB file. Restore the save folder afterwards. | The 1 GiB Offer is **not** auto-accepted: its sheet appears with "Needs … only … free" and Accept disabled. Decline, and Lin's row says "Win declined." The 1 MB file is auto-accepted and saves. | | | |
| WIN-5<br>[#55](https://github.com/theHimanshuShekhar/bhayanakshare/issues/55) | On Lin make a folder tree past 260 characters (below). Leave Windows' own long paths setting at its default, off (`Get-ItemPropertyValue -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -Name LongPathsEnabled` prints 0). Send `longtree` to Win, Accept. | No "Some paths are too long for this save folder" warning. Received. The file is saved whole at the full path, with the same SHA-256 as on Lin. Windows tools may not open a path this long by its plain name: list it with `Get-ChildItem -Recurse -File -LiteralPath '\\?\<save folder>\longtree'` and hash it with `Get-FileHash -LiteralPath '\\?\<the full path that shows>'`. | | | |
| WIN-6<br>[#56](https://github.com/theHimanshuShekhar/bhayanakshare/issues/56) | Win: append some text to the `one.bin` and `tree` already in the save folder (so they differ from the source) and note their SHA-256. Lin sends `one.bin` and `tree` again. Accept both. | Both are Received. The old `one.bin` and `tree` are untouched (same SHA-256 as before, no file merged into the old folder); the new ones are saved as `one (1).bin` and `tree (1)`. Nothing is overwritten. | | | |
| WIN-7 | Win, with the profile folder that has a space: with start at login on (the default; Settings → App), sign out of Windows and in again. Then switch it off in Settings → App and sign out and in again. | Before signing out, `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v BhayanakShare` shows the path in quotes, such as `"C:\Users\Test User\AppData\Local\BhayanakShare\bhayanakshare.exe" --background`. After sign-in the app is running with its window closed (a tray icon; a process in Task Manager), listed under Settings → Apps → Startup, and Lin can send it an Offer at once. With the setting off it does not start, and the Startup entry and the `reg query` value are gone. | | | |
| WIN-8 | Win: Settings → Diagnostics → Export diagnostics…, save the zip and open it. Expand it (`Expand-Archive`) and search the files for Lin's Device ID and for `one.bin` (`Select-String -SimpleMatch -Pattern <text> -Path .\about.txt, .\logs\*`). | The zip holds `logs/` and `about.txt`. `about.txt` names Windows and its build, such as `Windows 11 (build 26100), x86_64` (Windows 11 builds are 22000 and above; compare `winver`), the app version, Visibility and Win's Fingerprint. Lin's Device ID and `one.bin` appear nowhere; Lin appears only as a Fingerprint. | | | |

## Performance

[#48](https://github.com/theHimanshuShekhar/bhayanakshare/issues/48) found big Transfers slow on the CI runner. This row measures real hardware. The spec sets no figure ([section 10](spec/v1.md): LAN throughput is limited by disk, not protocol), so it passes when the Transfer completes and the time is recorded.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| PERF-1 | Lin → Win: send `big.bin` (1 GiB), Accept, and time it from Accept to "Received." | Completes with the same SHA-256. In Notes: the seconds, the rate the row showed, the link (wired gigabit, or the Wi-Fi band) and both PCs' disks (SSD or HDD). | | | |
| PERF-2 | Win → Lin: send the `big.bin` Win now holds, and time it the same way. | The same, the other way. A figure far below what the link and disk allow is written up as an issue; link it. | | | |

## Accessibility

The by-hand checklist is in [`docs/accessibility.md`](accessibility.md#before-a-release-by-hand). It is not repeated here: run the numbered item named in each row, on **both** the Windows build and the Linux AppImage (the page says "Linux AppImage at least"; the gate asks for both). Result is Pass only if both pass; write which one failed in Notes, and write the two builds in the Build column.

| ID | Steps | Expected | Result | Build | Notes |
|---|---|---|---|---|---|
| ACC-1 | Item 1, **keyboard only**, on Win and on Lin. | As that item says. | | | |
| ACC-2 | Item 2, **screen reader**, on Win and on Lin. The page names Orca (Linux) and VoiceOver (macOS, not in version 1): on Win use Narrator, and say so in Notes. | As that item says. | | | |
| ACC-3 | Item 3, **zoom to 200%**, on Win and on Lin. | As that item says. | | | |
| ACC-4 | Item 4, **contrast**, on Win and on Lin (on Win with a Windows contrast theme on, which is its forced-colours mode). | As that item says. | | | |

## Not in the gate

- **Internet and relay rows**: a Transfer over the internet, through n0's relays, n0 DNS or the public DHT. The relay is covered by automated tests: a Transfer through a rate-limited relay (`crates/core/tests/relay_limit.rs`) and the lookup in the public DHT (`crates/core/src/dht.rs`). Both run on demand rather than in CI ([README](../README.md#tests)), so the gate runs them as AUTO-1 and AUTO-2.
- **Windows ↔ Windows rows**: covered by Windows CI, which runs the Device API integration tests on Windows, with two Devices in one process.
- **macOS, ARM builds, and Linux distros other than Ubuntu 22.04+ and Fedora 39+.** Not part of version 1, or not checked (README, Install and update).
