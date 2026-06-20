# OpenWRT Package for CTRL-Modem

This directory contains the OpenWRT packaging files for the modem-interface application.

## Directory Structure

```
openwrt/
├── Makefile                          # OpenWRT SDK Makefile (for full SDK builds)
├── package-info/
│   ├── control                       # Package metadata (reference; CI generates dynamically)
│   └── conffiles                     # Config files to preserve on upgrade
└── files/                            # Files installed on the router
    ├── usr/
    │   ├── bin/
    │   │   └── modem-interface-update    # OTA update check/apply script
    │   └── share/luci/menu.d/
    │       └── luci-app-ctrl-modem.json  # LuCI Network menu entry
    ├── www/luci-static/resources/view/network/
    │   └── ctrl-modem.js                # LuCI view (redirect to CTRL-Modem)
    └── etc/
        ├── init.d/modem-interface        # Procd-based init script
        ├── config/modem-interface        # UCI configuration
        ├── cron.d/modem-interface-update  # Weekly auto-update cron job
        └── modem-interface/
            └── config.toml               # Application configuration
```

## Building the Package

### Via CI (Recommended)

The maintainer CI pipeline automatically:
1. Cross-compiles the backend for aarch64
2. Builds the frontend
3. Creates an .ipk package via `scripts/build-ipk.sh`
4. Publishes to the public package feed at packages.ctrl-modem.com

### Local .ipk Build (via WSL)

```powershell
.\scripts\build-ipk.ps1
```

### Manual .ipk Build

```bash
# 1. Build backend
cd backend && cargo zigbuild --release --target aarch64-unknown-linux-musl --features real-hardware

# 2. Build frontend
cd ../frontend && npm ci && npm run build

# 3. Create .ipk
./scripts/build-ipk.sh VERSION backend/target/aarch64-unknown-linux-musl/release/modem-interface frontend/dist/ .
```

### MIPS (`mipsel_24kc`, e.g. MT7621 routers on OpenWrt 22.03/opkg)

`mipsel-unknown-linux-musl` is a Rust Tier-3 target — the backend must be built with
`scripts/build-backend-mipsel.sh` (Linux x86_64 host, pinned nightly + `-Z build-std`,
OpenWrt 22.03 ramips/mt7621 SDK GCC; see the script header for the exact pins). Not in
CI yet (Item #31 Phase 5).

```bash
# 1. Build backend (frontend/dist must exist — it is embedded into the binary)
OPENWRT_SDK=/path/to/openwrt-sdk-22.03.7-ramips-mt7621_gcc-11.2.0_musl.Linux-x86_64 \
  sh scripts/build-backend-mipsel.sh

# 2. Create .ipk (frontend arg empty — embedded; arch is the 5th arg)
./scripts/build-ipk.sh VERSION \
  backend/target/mipsel-unknown-linux-musl/release-mipsel/modem-interface.stripped \
  "" . mipsel_24kc
```

**Size reality (measured on a Zbtlink ZBT-WG3526 16M, 2026-06-10, v1.3.0-dev.34):**
stripped binary 9,264,624 B; `.ipk` ~3.9 MB; **as-installed jffs2 footprint ~4.4 MB**
(jffs2 compresses on-flash). The `openssl-util` dependency chain (`libopenssl1.1` +
`libopenssl-conf`) adds ~1.3 MB on a clean 22.03 device. On a 16 MB-flash router
(~9.1 MB overlay) that leaves ~3 MB free after install — it fits, but check
`df -k /overlay` before installing on anything smaller. Serial kmods
(`kmod-usb-serial-option`, `kmod-usb-serial-wwan`) are intentionally NOT hard
dependencies — install them manually for USB modems.

## Installation on OpenWRT

### Via opkg from Feed (Preferred)

```bash
# One-time setup: add the public feed on the router
# Replace <arch> with your target (e.g. aarch64_cortex-a53, arm_cortex-a7_neon-vfpv4, mipsel_24kc)
echo "src/gz modem-interface https://packages.ctrl-modem.com/stable/feed/<arch>" >> /etc/opkg/customfeeds.conf

# Then install/upgrade:
opkg update
opkg install modem-interface    # first install
opkg upgrade modem-interface    # subsequent upgrades
```

### Via opkg from .ipk File

```bash
# Download the .ipk for your arch from the public feed directory:
#   https://packages.ctrl-modem.com/stable/feed/<arch>/
curl -o modem-interface.ipk "https://packages.ctrl-modem.com/stable/feed/<arch>/modem-interface_<version>_<arch>.ipk"

# Copy to router and install
scp -O modem-interface.ipk root@192.168.1.1:/tmp/
ssh root@192.168.1.1 "opkg install /tmp/modem-interface.ipk"
```

### Via Deploy Script

```powershell
# Direct file copy (no opkg)
.\scripts\deploy-to-router.ps1

# Via opkg feed
.\scripts\deploy-to-router.ps1 -UseOpkg
```

## Configuration

### UCI Configuration (`/etc/config/modem-interface`)

```
config modem-interface 'settings'
    option enabled '1'
    option listen_addr '0.0.0.0:8080'
    option signal_poll_interval '2'
    option log_level 'info'

config modem-interface 'network'
    option apn 'broadband'
    option auto_connect '0'

config modem-interface 'update'
    option auto_update '1'
    option check_interval '168'
```

To disable auto-updates: `uci set modem-interface.update.auto_update=0 && uci commit`

### Application Configuration (`/etc/modem-interface/config.toml`)

TOML format configuration file. See the file itself for all available options.

## First Run Setup

After installation, access CTRL-Modem at:
- **Direct:** `https://192.168.1.1:8443/ctrl-modem/home`
- **Via LuCI:** Network &rarr; CTRL-Modem (auto-redirects)

Accept the self-signed certificate on first visit. The first-run setup page will prompt you to create a root SuperAdmin account.

### Authentication

The interface supports three user roles:
- **SuperAdmin** — Full access, can manage all users
- **Admin** — Can manage ReadOnly users
- **ReadOnly** — View-only, restricted to assigned panels (default: Connection Status + Signal)

User accounts are stored in `/etc/modem-interface/users.json` and preserved across upgrades.

## Service Management

```bash
/etc/init.d/modem-interface start
/etc/init.d/modem-interface stop
/etc/init.d/modem-interface restart
/etc/init.d/modem-interface status
logread | grep modem-interface
```

## Dependencies

The package declares these dependencies:
- `libc` - C library
- `kmod-usb-serial` - USB serial driver
- `kmod-usb-serial-option` - USB modem driver
- `kmod-usb-serial-wwan` - WWAN USB driver
- `openssl-util` - Required for TLS certificate generation

## Package Contents

```
/usr/bin/modem-interface                              # Main binary (~7.4MB, frontend embedded)
/usr/bin/modem-interface-update                       # OTA update script
/etc/init.d/modem-interface                           # Procd init script
/etc/config/modem-interface                           # UCI config (preserved on upgrade)
/etc/cron.d/modem-interface-update                    # Weekly auto-update cron
/etc/modem-interface/config.toml                      # App config (preserved)
/etc/modem-interface/users.json                       # User accounts (preserved, created on first run)
/etc/modem-interface/tls/{cert,key}.pem               # TLS (auto-generated, preserved)
/usr/share/luci/menu.d/luci-app-ctrl-modem.json       # LuCI menu entry
/www/luci-static/resources/view/network/ctrl-modem.js # LuCI redirect view
```

Note: Frontend files are embedded in the binary — no separate `/www/` directory needed.

## Upgrading

Config files listed in `conffiles` are preserved during upgrades:
- `/etc/config/modem-interface` — UCI configuration
- `/etc/modem-interface/config.toml` — Application configuration
- `/etc/modem-interface/users.json` — User accounts and credentials
- `/etc/modem-interface/tls/cert.pem` — TLS certificate
- `/etc/modem-interface/tls/key.pem` — TLS private key

The prerm script stops the service before upgrade, and postinst restarts it after.

## Uninstallation

```bash
opkg remove modem-interface
```

## .ipk Format Notes

OpenWRT's opkg-lede expects .ipk files as **gzip-compressed tar archives** (not ar archives like Debian .deb). The outer archive contains `./debian-binary`, `./control.tar.gz`, and `./data.tar.gz`. All tars use `--format=gnu` to match OpenWRT's ipkg-build conventions.

## Auto-Updates

The package includes a pull-based OTA update system. The router checks the opkg feed for new versions and can apply updates automatically or via the web UI.

### Automatic (Cron)

A weekly cron job runs Sunday at 3am with random jitter:
```bash
# /etc/cron.d/modem-interface-update
0 3 * * 0 root sleep $(( RANDOM % 300 )) && /usr/bin/modem-interface-update apply
```

Disable via UCI: `uci set modem-interface.update.auto_update=0 && uci commit`

### Manual (Web UI)

Enable the "System Update" panel in the sidebar, then click "Check for Updates". If an update is available, click "Apply Update" — the service restarts automatically and the page reloads with the new version.

### Manual (CLI)

```bash
modem-interface-update version   # Show installed and available versions (JSON)
modem-interface-update check     # Check if update available (exit 0=yes, 1=no, 2=error)
modem-interface-update apply     # Check and apply update if available
```

Logs: `/var/log/modem-interface-update.log`

## Troubleshooting

### Service won't start

```bash
ls -la /usr/bin/modem-interface
/usr/bin/modem-interface              # Run manually to see errors
logread | tail -50
```

### Can't access web interface

```bash
ps | grep modem-interface
netstat -tlnp | grep 8080
curl http://localhost:8080/ctrl-modem/api/modem/status
```

### Modem not detected

```bash
lsusb
ls -la /dev/ttyUSB*
lsmod | grep usb_serial
```

## Support

- Source & issues: https://github.com/DidneyWhorl/modem-interface
- Documentation: see the `docs/` directory and this README
