# Allowing local discovery through a firewall

BhayanakShare finds Nearby Devices with mDNS: Devices send small multicast packets to `224.0.0.251` (and `ff02::fb`) on **UDP port 5353**. If a firewall drops those packets, no Nearby Devices appear, and Home suggests this page after 30 seconds. If BhayanakShare cannot use the port at all, Home says so at once instead: see [When Home says local discovery could not start](#when-home-says-local-discovery-could-not-start). Sending to a Device by its Device ID still works over the internet.

A Device shows up for others according to its Visibility (Settings): to everyone on **Everyone**, only to Devices that already have its Device ID on **People who have my ID** (the default), and to nobody on **Hidden**, which announces nothing, does not look for Devices either (so it lists none), and only answers a Device that asks for it by its Device ID. The question and the answer are both mDNS multicast, so the rule for mDNS below is all it needs. Check that on the Device you expect to see, and that this Device has the other's ID saved as a Contact if it is not on Everyone, before changing a firewall.

Two things must be allowed:

1. **mDNS, inbound**: UDP port 5353, so Devices hear each other.
2. **The app itself, inbound**: BhayanakShare listens on a UDP port it picks at random each time it starts, so allow the program rather than a port number. Without this, a Transfer between two Devices on one network falls back to the internet relay, or fails offline.

BhayanakShare never changes firewall rules itself, and neither do its installers or packages, on Linux or Windows: the AppImage, deb and rpm run no install, update or removal scripts, and the Windows installer adds no firewall rules, so installing, updating or removing it leaves your firewall as it was. Allowing the two things above is always something you do yourself. On Windows, Windows asks you the first time, see [Windows](#windows).

The steps below come from each system's documentation. The project's tests run on Linux and Windows, but not against a firewall, and the steps have not been tried on every system and version listed.

## When Home says local discovery could not start

This is not the firewall guess above: BhayanakShare tried to open the mDNS port and could not, on Windows and on Linux alike. Home says why, in one of three ways:

- **The port is in use or refused.** Another program holds UDP port 5353 and does not share it (most mDNS software shares it, as Windows' own mDNS, browsers and Avahi do), or the system or security software refused the program. Close the other program or stop its mDNS, or allow BhayanakShare in the security software, and check the two firewall steps in [Linux](#linux) or [Windows](#windows) below.
- **No network connection can use it.** No network interface is up, or none supports multicast. Connect to a network. A VPN that takes over all traffic can leave nothing else (see Other causes below).
- **The system reported an error.** The line in the log says which: Settings, Diagnostics, Export diagnostics.

Nothing needs a restart: BhayanakShare tries again every 15 seconds, and also whenever the Visibility, the Device Name or the network address changes, each time looking at the network connections as they are then (so one that came up since is used), and the hint goes away by itself when it works. Home reports a problem only when local discovery cannot run at all: if the port can be used on IPv6 but not on IPv4, discovery runs on IPv6 and BhayanakShare keeps trying for IPv4. Until then the Device finds nobody Nearby and nobody finds it. A **Hidden** Device also cannot answer a Device that asks for it by its Device ID, so people who have its ID cannot reach it on the local network; sending to it over the internet still works. Sending to a Device by its Device ID works whatever happens here.

## Linux

**firewalld** (Fedora, RHEL, openSUSE): the `public` zone does not allow mDNS; the `home` zone does. See which zone your network is in, then either put it in the `home` zone, or allow the `mdns` service (UDP port 5353, `224.0.0.251` and `ff02::fb`) in the zone it is in:

```sh
sudo firewall-cmd --get-active-zones   # which zone each network interface is in

# Either: move the network to the home zone (with NetworkManager; "Wired connection 1" is
# the connection's name, see `nmcli connection show`)
sudo nmcli connection modify "Wired connection 1" connection.zone home
sudo nmcli connection up "Wired connection 1"

# Or: allow mDNS in the zone it is already in (here public)
sudo firewall-cmd --zone=public --add-service=mdns             # until the next reload
sudo firewall-cmd --permanent --zone=public --add-service=mdns # and keep it
sudo firewall-cmd --reload
sudo firewall-cmd --zone=public --list-services                # mdns should be in the list
```

Without `--zone`, `firewall-cmd` changes the default zone, which is not always the one your network is in. The same command works for any zone, such as `--zone=FedoraWorkstation` on Fedora.

firewalld has no per-program rules, and the app's UDP port changes on each start. If Devices see each other but Transfers go through the relay or fail offline, allow traffic from your local network as a whole, in the zone your network is in (`<your zone>`, as `--get-active-zones` showed). Change the IPv4 range to yours, and add the IPv6 rules too if your network has IPv6, as Devices may use either (`fe80::/10` is the link-local range every IPv6 network has; use your network's own prefix as well, shown by `ip -6 addr`, if it has one):

```sh
sudo firewall-cmd --permanent --zone=<your zone> --add-rich-rule='rule family="ipv4" source address="192.168.1.0/24" accept'
sudo firewall-cmd --permanent --zone=<your zone> --add-rich-rule='rule family="ipv6" source address="fe80::/10" accept'
sudo firewall-cmd --reload
```

This lets every Device on that network reach every port on this one, so use it only on a network you trust.

**ufw** (Ubuntu, Debian): ufw's stock rules (`/etc/ufw/before.rules`) normally accept mDNS already. If yours do not, allow it, and allow your local network for the Transfers themselves:

```sh
sudo ufw allow in proto udp to 224.0.0.251 port 5353
sudo ufw allow in proto udp from 192.168.1.0/24
sudo ufw allow in proto udp from fe80::/10
```

If you run Avahi, nothing needs changing for BhayanakShare: both share port 5353.

## Windows

The first time BhayanakShare listens on the network, which is when it starts, Windows Defender Firewall shows "Windows Defender Firewall has blocked some features of this app". The installer does not trigger it and does not change any rule: Windows asks because no rule for the program exists yet.

**Answer it as an administrator, with Allow.**

1. Tick **Private networks** (a home or office network you trust). Leave **Public networks** unticked unless this network is marked Public and you trust it.
2. Choose **Allow access**.

Only an administrator can allow it. **Cancelling the prompt, or answering it as a standard user, makes Windows create persistent block rules for the program** (one for TCP and one for UDP), and **the prompt never comes back**, so the app stays blocked without saying so. Block rules win over allow rules. Symptoms are the ones in the first section: no Nearby Devices, and Transfers that take the relay or fail offline.

**To undo a block**, as an administrator:

1. Open Windows Security, **Firewall & network protection**, **Advanced settings**, then **Inbound Rules**. Remove the rules for BhayanakShare: look for rules whose Action is Block and whose Program is BhayanakShare's program file (sort by Name or Program). There are usually two, one for TCP and one for UDP. (Or run `wf.msc`.)
2. Start BhayanakShare again. Windows asks again, as no rule exists: answer as above. Or allow it yourself: Windows Security, Firewall & network protection, **Allow an app through firewall**, **Change settings**, **Allow another app**, choose BhayanakShare's program file (the installer put it in a folder under `%LOCALAPPDATA%` unless you chose another), **Add**, and tick **Private**.

If the network is marked **Public**, Windows treats it as untrusted and the rule for Private networks does not apply. Set the network to Private (Settings, Network and internet, your network, Network profile type), or tick **Public** for the app in "Allow an app through firewall".

**mDNS** is UDP port 5353, as above. The rule Windows makes from its prompt is for the program on any port, so it covers mDNS and no separate port rule is needed. Windows' own mDNS service and browsers use the same port and share it with BhayanakShare, so they do not get in its way. If another program holds it without sharing, see [When Home says local discovery could not start](#when-home-says-local-discovery-could-not-start).

**Why the rule is for the program, not a port:** BhayanakShare listens on a UDP port it picks at random each time it starts, so a rule for one port would stop working at the next start. A rule for the program holds whatever port it picks.

On a computer managed by an organisation, Group Policy may stop programs from adding firewall rules or switch the prompt off. Then an administrator must add the rule.

These steps follow Microsoft's pages [Windows Firewall rules](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/rules) and [Risks of allowing apps through Windows Firewall](https://support.microsoft.com/en-us/windows/security/firewall/risks-of-allowing-apps-through-windows-firewall); they have not been tried on every Windows 11 build.

## macOS

Not supported in version 1.

## Other causes

- **Wi-Fi client isolation** (often called "guest network" or "AP isolation") stops Devices on the same Wi-Fi from reaching each other, and mDNS with them. Use another network, or send by Device ID.
- **VPNs** that route all traffic can move LAN traffic off the local network. Disconnect, or choose split tunnelling.
- **Different networks or subnets**: mDNS does not cross routers.
