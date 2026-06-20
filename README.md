# CTRL-Modem

**A modern web UI for cellular modems on OpenWRT. Does not work inside LuCI, so you don't need LuCI installed.**

CTRL-Modem is a standalone web interface for managing cellular (4G/5G) modems on
OpenWRT routers. It replaces LuCI's network screens for modem management with a
fast, real-time dashboard — install once, browse to your router, and manage every
modem from a single screen.

![CTRL-Modem dashboard](docs/assets/ui.gif)
<!-- TODO: add docs/assets/ui.gif -->

## Features

- **Real-time signal monitoring** — live RSRP, RSRQ, and SINR, plus active bands
  and connection technology, updated continuously without a manual refresh.
- **Connection & WAN management** — bring connections up and down, with multi-WAN
  failover so traffic moves to a healthy link automatically.
- **SIM information** — SIM status and identifiers (ICCID, IMSI, IMEI) at a glance.
- **GPS location** — current position when the modem supports location services.
- **Antenna metrics & carrier aggregation** — a dedicated view of antenna metrics
  and the active carrier-aggregation layout, with on-demand live polling at
  selectable intervals.
- **APN / PDP profile management** — create and edit data profiles directly from
  the dashboard.
- **Multi-modem support** — manage several modems on one router, each with its own
  status and controls.
- **Role-based access** — three built-in roles (SuperAdmin, Admin, ReadOnly) so you
  can grant exactly the access each person needs.
- **Single binary** — the entire interface runs as one self-contained service
  directly on the router.

## Supported hardware

| Type   | Device                                                       | Notes          |
| ------ | ------------------------------------------------------------ | -------------- |
| Router | BananaPi BPI-R4-PRO (aarch64)                                | Primary target |
| Router | MT7621-class routers (`mipsel_24kc`), e.g. Zbtlink ZBT-WG3526 | Supported      |
| Modem  | Quectel RM551E-GL                                            |                |
| Modem  | Quectel RM520N-GL                                            |                |
| Modem  | Telit FN990                                                  |                |

Other Quectel and Telit USB modems that expose a standard serial interface are
likely to work as well.

## Install

Current stable version: **1.3.0**

On your OpenWRT router, add the CTRL-Modem package feed and install:

```sh
# Add the CTRL-Modem stable feed
echo 'src/gz ctrl_modem https://packages.ctrl-modem.com/stable/' >> /etc/opkg/customfeeds.conf

# Update package lists and install
opkg update
opkg install modem-interface

# Optional: add the menu entry to the LuCI menu
opkg install luci-app-ctrl-modem
```

## First run

After installation, open the interface in your browser:

```
https://192.168.1.1:8443/ctrl-modem/home
```

The interface is served over HTTPS with a self-signed certificate — accept the
certificate warning to continue. On first launch you'll land on the setup page,
where you create the root **SuperAdmin** account. That's it — you're ready to
manage your modems.

## CTRL-Cloud (optional)

CTRL-Modem is fully functional on its own. **Every local feature is free and works
with no license and no account** — just install it and use it.

For operators running fleets of devices, **CTRL-Cloud** is an optional paid managed
service that adds remote management and centralized licensing across many routers
from a single place. It's entirely optional: the local app never requires the
cloud, a license, or an account to do its job.

Learn more at **[portal.ctrl-modem.com](https://portal.ctrl-modem.com)**.

## License

CTRL-Modem is source-available under the **Business Source License 1.1** (see
[`LICENSE`](LICENSE)). On the Change Date specified in the license, it converts to
the **Apache License 2.0**.

## Contributing

Contributions are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md) to get started.
