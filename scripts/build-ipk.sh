#!/bin/sh
# build-ipk.sh - Build OpenWRT .ipk packages from pre-built artifacts
#
# Produces 2 packages:
#   modem-interface          Core (arch-specific binary + config)
#   luci-app-ctrl-modem      LuCI integration (arch: all)
#
# Usage: ./scripts/build-ipk.sh <version> <binary-path> [frontend-dist-dir] [output-dir] [arch]
#
# Example (CI):
#   ./scripts/build-ipk.sh 1.0.77 modem-interface-aarch64 "" . aarch64_cortex-a53
#   ./scripts/build-ipk.sh 1.0.77 modem-interface-armv7 "" . arm_cortex-a7_neon-vfpv4
#   ./scripts/build-ipk.sh 1.0.77 modem-interface-mipsel "" . mipsel_24kc
# Example (local via WSL):
#   ./scripts/build-ipk.sh 1.0.77 backend/target/aarch64-unknown-linux-musl/release/modem-interface frontend/dist/ .

set -e

VERSION="${1:?Usage: build-ipk.sh <version> <binary-path> [frontend-dist-dir] [output-dir] [arch]}"
BINARY_PATH="${2:?Missing binary path}"
FRONTEND_DIR="${3:-}"
OUTPUT_DIR="${4:-.}"
ARCH="${5:-aarch64_cortex-a53}"

PKG_RELEASE="1"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OPKG_VERSION="$("$SCRIPT_DIR/version-translate.sh" opkg "$VERSION")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
WORK_DIR=$(mktemp -d)

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# --- Helper: assemble an .ipk from a prepared data dir and control dir ---
# Args: $1=package-name $2=data-dir $3=control-dir $4=architecture $5=output-filename
assemble_ipk() {
    local pkg_name="$1"
    local data_dir="$2"
    local ctrl_dir="$3"
    local pkg_arch="$4"
    local ipk_filename="$5"

    local ipk_work=$(mktemp -d)

    # Calculate installed size in KB
    local installed_size=$(du -sk "$data_dir" | cut -f1)

    # Inject Installed-Size into control file
    sed -i "s/^Installed-Size: .*/Installed-Size: ${installed_size}/" "${ctrl_dir}/control"

    # Build data.tar.gz
    cd "$data_dir"
    tar --format=gnu --numeric-owner --sort=name --owner=0 --group=0 --mtime="@0" -cf - . | gzip -n - > "${ipk_work}/data.tar.gz"
    cd "$OLDPWD"

    # Build control.tar.gz
    cd "$ctrl_dir"
    tar --format=gnu --numeric-owner --sort=name --owner=0 --group=0 --mtime="@0" -cf - . | gzip -n - > "${ipk_work}/control.tar.gz"
    cd "$OLDPWD"

    # debian-binary
    echo "2.0" > "${ipk_work}/debian-binary"

    # Assemble .ipk (gzip-compressed tar, OpenWRT opkg-lede format)
    cd "$ipk_work"
    tar --format=gnu --numeric-owner --sort=name --mtime="@0" -cf - ./debian-binary ./data.tar.gz ./control.tar.gz | gzip -n - > "$ipk_filename"
    cd "$OLDPWD"

    mkdir -p "$OUTPUT_DIR"
    cp "${ipk_work}/${ipk_filename}" "${OUTPUT_DIR}/${ipk_filename}"
    rm -rf "$ipk_work"

    echo "  -> ${OUTPUT_DIR}/${ipk_filename} ($(du -h "${OUTPUT_DIR}/${ipk_filename}" | cut -f1))"
}


# ============================================================
# Package 1: modem-interface (core)
# ============================================================
echo ""
echo "=== Building modem-interface (core) ==="

CORE_IPK="modem-interface_${OPKG_VERSION}-${PKG_RELEASE}_${ARCH}.ipk"
CORE_DATA="${WORK_DIR}/core-data"
CORE_CTRL="${WORK_DIR}/core-control"

# --- Core data ---
mkdir -p "${CORE_DATA}/usr/bin"
mkdir -p "${CORE_DATA}/etc/init.d"
mkdir -p "${CORE_DATA}/etc/config"
mkdir -p "${CORE_DATA}/etc/modem-interface"
mkdir -p "${CORE_DATA}/etc/modem-interface/tls"
if [ -n "$FRONTEND_DIR" ]; then
    mkdir -p "${CORE_DATA}/www/modem-interface"
fi

# Binary
cp "$BINARY_PATH" "${CORE_DATA}/usr/bin/modem-interface"
chmod 755 "${CORE_DATA}/usr/bin/modem-interface"

# Init script
cp "${PROJECT_ROOT}/openwrt/files/etc/init.d/modem-interface" "${CORE_DATA}/etc/init.d/modem-interface"
chmod 755 "${CORE_DATA}/etc/init.d/modem-interface"

# Config files
cp "${PROJECT_ROOT}/openwrt/files/etc/config/modem-interface" "${CORE_DATA}/etc/config/modem-interface"
chmod 644 "${CORE_DATA}/etc/config/modem-interface"

cp "${PROJECT_ROOT}/openwrt/files/etc/modem-interface/config.toml" "${CORE_DATA}/etc/modem-interface/config.toml"
chmod 644 "${CORE_DATA}/etc/modem-interface/config.toml"

# Frontend files (skipped when frontend is embedded in binary)
if [ -n "$FRONTEND_DIR" ]; then
    cp -r "${FRONTEND_DIR}"/* "${CORE_DATA}/www/modem-interface/"
fi

# Update script
if [ -f "${PROJECT_ROOT}/openwrt/files/usr/bin/modem-interface-update" ]; then
    cp "${PROJECT_ROOT}/openwrt/files/usr/bin/modem-interface-update" "${CORE_DATA}/usr/bin/modem-interface-update"
    chmod 755 "${CORE_DATA}/usr/bin/modem-interface-update"
fi

# Cron job for auto-updates
if [ -f "${PROJECT_ROOT}/openwrt/files/etc/cron.d/modem-interface-update" ]; then
    mkdir -p "${CORE_DATA}/etc/cron.d"
    cp "${PROJECT_ROOT}/openwrt/files/etc/cron.d/modem-interface-update" "${CORE_DATA}/etc/cron.d/modem-interface-update"
    chmod 644 "${CORE_DATA}/etc/cron.d/modem-interface-update"
fi

# --- Core control ---
mkdir -p "$CORE_CTRL"

cat > "${CORE_CTRL}/control" <<CTRL
Package: modem-interface
Version: ${OPKG_VERSION}-${PKG_RELEASE}
Depends: openssl-util, ca-bundle
Recommends: luci-app-ctrl-modem
Source: ctrl-modem
Section: net
Architecture: ${ARCH}
Installed-Size: 0
Maintainer: agccc <agccc@netsolution.shop>
Description: CTRL-Modem — cellular modem management interface
 A standalone web interface for managing cellular modems on OpenWRT routers.
 Part of the CTRL-Modem suite. Supports 4G/5G modems with real-time signal
 monitoring, connection management, and multi-WAN failover.
CTRL

cp "${PROJECT_ROOT}/openwrt/package-info/conffiles" "${CORE_CTRL}/conffiles"

cat > "${CORE_CTRL}/postinst" <<'POSTINST'
#!/bin/sh
[ -n "${IPKG_INSTROOT}" ] || {
    echo "Enabling modem-interface service..."
    /etc/init.d/modem-interface enable
    /etc/init.d/modem-interface start
    # Restart cron to pick up new cron.d entry for auto-updates
    /etc/init.d/cron restart 2>/dev/null || true
    ROUTER_IP=$(uci -q get network.lan.ipaddr | cut -d/ -f1)
    ROUTER_IP=${ROUTER_IP:-192.168.1.1}
    echo ""
    echo "CTRL-Modem is running."
    echo "  Access: https://${ROUTER_IP}:8443/ctrl-modem/"
    if opkg status luci-app-ctrl-modem 2>/dev/null | grep -q "^Status:.*installed"; then
        echo "  LuCI menu: Network -> CTRL-Modem"
    fi
    echo ""
}
exit 0
POSTINST
chmod 755 "${CORE_CTRL}/postinst"

cat > "${CORE_CTRL}/prerm" <<'PRERM'
#!/bin/sh
[ -n "${IPKG_INSTROOT}" ] || {
    echo "Stopping modem-interface service..."
    /etc/init.d/modem-interface stop
    /etc/init.d/modem-interface disable
}
exit 0
PRERM
chmod 755 "${CORE_CTRL}/prerm"

assemble_ipk "modem-interface" "$CORE_DATA" "$CORE_CTRL" "$ARCH" "$CORE_IPK"


# ============================================================
# Package 2: luci-app-ctrl-modem (LuCI integration)
# ============================================================
echo ""
echo "=== Building luci-app-ctrl-modem ==="

LUCI_IPK="luci-app-ctrl-modem_${OPKG_VERSION}-${PKG_RELEASE}_all.ipk"
LUCI_DATA="${WORK_DIR}/luci-data"
LUCI_CTRL="${WORK_DIR}/luci-control"

# --- LuCI data ---
# JSON menu entry (LuCI 22.03+)
mkdir -p "${LUCI_DATA}/usr/share/luci/menu.d"
cp "${PROJECT_ROOT}/openwrt/files/usr/share/luci/menu.d/luci-app-ctrl-modem.json" \
   "${LUCI_DATA}/usr/share/luci/menu.d/luci-app-ctrl-modem.json"
chmod 644 "${LUCI_DATA}/usr/share/luci/menu.d/luci-app-ctrl-modem.json"

# JS redirect view (LuCI 22.03+)
mkdir -p "${LUCI_DATA}/www/luci-static/resources/view/network"
cp "${PROJECT_ROOT}/openwrt/files/www/luci-static/resources/view/network/ctrl-modem.js" \
   "${LUCI_DATA}/www/luci-static/resources/view/network/ctrl-modem.js"
chmod 644 "${LUCI_DATA}/www/luci-static/resources/view/network/ctrl-modem.js"

# Lua controller (LuCI 19.07-21.02)
mkdir -p "${LUCI_DATA}/usr/lib/lua/luci/controller"
cp "${PROJECT_ROOT}/openwrt/files/usr/lib/lua/luci/controller/ctrl-modem.lua" \
   "${LUCI_DATA}/usr/lib/lua/luci/controller/ctrl-modem.lua"
chmod 644 "${LUCI_DATA}/usr/lib/lua/luci/controller/ctrl-modem.lua"

# Lua redirect template (LuCI 19.07-21.02)
mkdir -p "${LUCI_DATA}/usr/lib/lua/luci/view"
cp "${PROJECT_ROOT}/openwrt/files/usr/lib/lua/luci/view/ctrl_modem_redirect.htm" \
   "${LUCI_DATA}/usr/lib/lua/luci/view/ctrl_modem_redirect.htm"
chmod 644 "${LUCI_DATA}/usr/lib/lua/luci/view/ctrl_modem_redirect.htm"

# --- LuCI control ---
mkdir -p "$LUCI_CTRL"

cat > "${LUCI_CTRL}/control" <<CTRL
Package: luci-app-ctrl-modem
Version: ${OPKG_VERSION}-${PKG_RELEASE}
Depends: modem-interface, luci-base
Source: ctrl-modem
Section: luci
Architecture: all
Installed-Size: 0
Maintainer: agccc <agccc@netsolution.shop>
Description: CTRL-Modem — LuCI menu integration
 Adds a CTRL-Modem entry to the LuCI web interface under Network.
 Part of the CTRL-Modem suite. Supports LuCI 19.07+ (Lua) and 22.03+ (JSON).
CTRL

assemble_ipk "luci-app-ctrl-modem" "$LUCI_DATA" "$LUCI_CTRL" "all" "$LUCI_IPK"


# ============================================================
# Summary
# ============================================================
echo ""
echo "=== All packages built successfully ==="
echo "Core:  ${OUTPUT_DIR}/${CORE_IPK}"
echo "LuCI:  ${OUTPUT_DIR}/${LUCI_IPK}"
