# Allowing local discovery through a firewall

BhayanakShare finds Nearby Devices with mDNS: Devices send small multicast packets to `224.0.0.251` (and `ff02::fb`) on **UDP port 5353**. If a firewall drops those packets, no Nearby Devices appear, and Home suggests this page after 30 seconds. Sending to a Device by its Device ID still works over the internet.

A Device only shows up for others when its Visibility is **Everyone** (Settings). Check that on the Device you expect to see before changing a firewall.

Two things must be allowed:

1. **mDNS, inbound**: UDP port 5353, so Devices hear each other.
2. **The app itself, inbound**: BhayanakShare listens on a UDP port it picks at random each time it starts, so allow the program rather than a port number. Without this, a Transfer between two Devices on one network falls back to the internet relay, or fails offline.

BhayanakShare never changes firewall rules itself.

The steps below come from each system's documentation. The project tests on Linux only, and they have not been run on every system listed.

## Linux

**firewalld** (Fedora, RHEL, openSUSE): the `public` zone does not allow mDNS; the `home` zone does. Either put the network in the `home` zone, or allow the service in the zone it is in:

```sh
sudo firewall-cmd --zone=public --add-service=mdns             # until the next reload
sudo firewall-cmd --permanent --zone=public --add-service=mdns # and keep it
sudo firewall-cmd --reload
```

firewalld has no per-program rules, and the app's UDP port changes on each start. If Devices see each other but Transfers go through the relay or fail offline, allow traffic from your local network as a whole, for example (change the range to yours):

```sh
sudo firewall-cmd --permanent --zone=public --add-rich-rule='rule family="ipv4" source address="192.168.1.0/24" accept'
sudo firewall-cmd --reload
```

This lets every Device on that network reach every port on this one, so use it only on a network you trust.

**ufw** (Ubuntu, Debian): ufw's stock rules (`/etc/ufw/before.rules`) normally accept mDNS already. If yours do not, allow it, and allow your local network for the Transfers themselves:

```sh
sudo ufw allow in proto udp to 224.0.0.251 port 5353
sudo ufw allow in proto udp from 192.168.1.0/24
```

If you run Avahi, nothing needs changing for BhayanakShare: both share port 5353.

## Windows

The first time BhayanakShare runs, Windows Defender Firewall asks whether to allow it. Allow it on **Private** networks. If the network is marked **Public**, Windows blocks inbound traffic: set the network to Private (Settings, Network and internet, your network, Network profile type), or allow the app on Public networks in "Allow an app through Windows Firewall".

## macOS

macOS 15 and later ask for **Local Network** access the first time the app looks for Devices. Choose Allow. If you refused, turn it on in System Settings, Privacy and Security, Local Network.

## Other causes

- **Wi-Fi client isolation** (often called "guest network" or "AP isolation") stops Devices on the same Wi-Fi from reaching each other, and mDNS with them. Use another network, or send by Device ID.
- **VPNs** that route all traffic can move LAN traffic off the local network. Disconnect, or choose split tunnelling.
- **Different networks or subnets**: mDNS does not cross routers.
