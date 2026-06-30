# CTRL-Modem

**A modern web UI for cellular modems on OpenWRT. Does not work inside LuCI, so you don't need LuCI installed.**

CTRL-Modem is a standalone web interface for managing cellular (4G/5G) modems on
OpenWRT routers. It replaces LuCI's network screens for modem management with a
fast, real-time dashboard — install once, browse to your router, and manage every
modem from a single screen.

![CTRL-Modem dashboard](docs/assets/ui.gif)
<!-- TODO: add docs/assets/ui.gif -->

## A Note From Didney

**Talk about it on Discord here: https://discord.gg/b8qJC3NKv**

**Full transparency up front.**

Didney here. I want to be sure there is no confusion on this project, as I
believe in transparency when it comes to capability and work.
I completely designed the plan for this project, and worked for months
(started in about Feb 2026 I believe) to get this project to it's current point
(writing this on 06/25/2026). Every part of the application I iterated the ideas
and layout, and useful functions, and tested on every thing I could. The stack
for it all I chose based on how I believe(d) the specific parts would perform for
the needs I wanted to fulfill.
The CODE I wrote 0% of. NONE. All was done with LLMs. Mostly Claude, as it evolved
over the last 4-5 months, but some other LLMs sprinkled in from time to time.
I do not in any way want to represent the code as my own knowledge created work.
I don't have the skills in that area, yet. I did on the other hand use my knowledge
with cellular modems and connectivity and such to help guide the functionality.
I hope you can use what "I've" made here, and I welcome any and all feedback.

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

Current stable version: **1.5.0**

OpenWRT ships one of two package managers depending on its version: **`opkg`**
(OpenWRT 23.05 and earlier) and **`apk`** (apk-tools 3.x, OpenWRT 24.10 and
newer). Use the block that matches your router. In both cases, replace `<arch>`
with your router's architecture (e.g. `aarch64_cortex-a53`,
`arm_cortex-a7_neon-vfpv4`, `mipsel_24kc`).

### With opkg (OpenWRT 23.05 and earlier)

```sh
# Add the CTRL-Modem stable feed
echo 'src/gz ctrl_modem https://packages.ctrl-modem.com/stable/feed/<arch>' >> /etc/opkg/customfeeds.conf

# Update package lists and install
opkg update
opkg install modem-interface

# Optional: add the menu entry to the LuCI menu
opkg install luci-app-ctrl-modem
```

### With apk (OpenWRT 24.10 and newer)

```sh
# Trust the feed signing key (apk verifies packages against keys in /etc/apk/keys/)
wget -O /etc/apk/keys/ctrl-modem-ec.pem https://packages.ctrl-modem.com/stable/apk/ctrl-modem-ec.pem

# Add the CTRL-Modem stable feed (apk auto-appends your router's <arch>)
echo 'https://packages.ctrl-modem.com/stable/apk' >> /etc/apk/repositories.d/customfeeds.list

# Update package lists and install
apk update
apk add modem-interface

# Optional: add the menu entry to the LuCI menu
apk add luci-app-ctrl-modem
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

## Acknowledgments

Every line of CTRL-Modem's source code was written by **Claude**, Anthropic's AI
assistant, under the design and direction of Didney — see
[*A Note From Didney*](#a-note-from-didney) above for the full story. Most of the
work was done with Claude; other LLMs contributed from time to time.

— [anthropic.com](https://www.anthropic.com)
