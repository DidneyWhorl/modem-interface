#!/bin/sh
# build-apk.sh - Build OpenWRT .apk v3 packages using apk mkpkg
#
# Produces 2 packages:
#   modem-interface          Core (arch-specific binary + config)
#   luci-app-ctrl-modem      LuCI integration (arch: noarch)
#
# Requires: apk-tools 3.x (Alpine edge) for `apk mkpkg`
#
# Usage: ./scripts/build-apk.sh <version> <binary-path> [frontend-dist-dir] [output-dir] [arch] [-k signing-key]
#
# Example (CI):
#   ./scripts/build-apk.sh 1.0.140 modem-interface-aarch64 "" . aarch64_cortex-a53
# Example (signed):
#   ./scripts/build-apk.sh 1.0.140 modem-interface-aarch64 "" . aarch64_cortex-a53 -k /path/to/key.pem

set -e

VERSION="${1:?Usage: build-apk.sh <version> <binary-path> [frontend-dist-dir] [output-dir] [arch] [-k signing-key]}"
BINARY_PATH="${2:?Missing binary path}"
FRONTEND_DIR="${3:-}"
OUTPUT_DIR="${4:-.}"
ARCH="${5:-aarch64}"
SIGNING_KEY=""

# Parse optional -k flag (may appear as $6 $7)
shift 5 2>/dev/null || true
while [ $# -gt 0 ]; do
    case "$1" in
        -k)
            SIGNING_KEY="${2:?Missing signing key path after -k}"
            shift 2
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# Fall back to APK_RSA_KEY env var if -k was not provided
[ -z "$SIGNING_KEY" ] && SIGNING_KEY="${APK_RSA_KEY:-}"

PKG_RELEASE="1"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APK_VERSION="$("$SCRIPT_DIR/version-translate.sh" apk "$VERSION")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
WORK_DIR=$(mktemp -d)

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# Verify apk mkpkg is available (--help returns non-zero on apk-tools, so check output instead)
if ! apk mkpkg --help 2>&1 | grep -q "mkpkg"; then
    echo "ERROR: 'apk mkpkg' not found. Requires apk-tools 3.x (Alpine edge)." >&2
    exit 1
fi

# ============================================================
# Package 1: modem-interface (core)
# ============================================================
echo ""
echo "=== Building modem-interface (core) ==="

CORE_APK="modem-interface-${APK_VERSION}-r${PKG_RELEASE}.apk"
CORE_DATA="${WORK_DIR}/core-data"

# --- Core staging directory (files at full install paths) ---
mkdir -p "${CORE_DATA}/usr/bin"
mkdir -p "${CORE_DATA}/etc/init.d"
mkdir -p "${CORE_DATA}/etc/config"
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

# --- Core post-install script ---
CORE_POSTINST="${WORK_DIR}/core-postinst"
cat > "$CORE_POSTINST" <<'POSTINST'
#!/bin/sh
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
if apk info -e luci-app-ctrl-modem >/dev/null 2>&1; then
    echo "  LuCI menu: Network -> CTRL-Modem"
fi
echo ""
exit 0
POSTINST
chmod 755 "$CORE_POSTINST"

# --- Core pre-deinstall script ---
CORE_PRERM="${WORK_DIR}/core-prerm"
cat > "$CORE_PRERM" <<'PRERM'
#!/bin/sh
echo "Stopping modem-interface service..."
/etc/init.d/modem-interface stop
/etc/init.d/modem-interface disable
exit 0
PRERM
chmod 755 "$CORE_PRERM"

# --- Build core package with apk mkpkg ---
SIGN_ARGS=""
if [ -n "$SIGNING_KEY" ] && [ -f "$SIGNING_KEY" ]; then
    SIGN_ARGS="--sign $SIGNING_KEY"
fi

mkdir -p "$OUTPUT_DIR"

# shellcheck disable=SC2086
apk mkpkg \
    --info "name:modem-interface" \
    --info "version:${APK_VERSION}-r${PKG_RELEASE}" \
    --info "description:CTRL-Modem — cellular modem management interface" \
    --info "arch:${ARCH}" \
    --info "license:proprietary" \
    --info "url:https://ctrl-modem.com" \
    --info "maintainer:agccc <agccc@netsolution.shop>" \
    --info "depends:ca-bundle" \
    --script "post-install:${CORE_POSTINST}" \
    --script "pre-deinstall:${CORE_PRERM}" \
    --files "${CORE_DATA}" \
    --output "${OUTPUT_DIR}/${CORE_APK}" \
    $SIGN_ARGS

echo "  -> ${OUTPUT_DIR}/${CORE_APK} ($(du -h "${OUTPUT_DIR}/${CORE_APK}" | cut -f1))"


# ============================================================
# Package 2: luci-app-ctrl-modem (LuCI integration)
# ============================================================
echo ""
echo "=== Building luci-app-ctrl-modem ==="

LUCI_APK="luci-app-ctrl-modem-${APK_VERSION}-r${PKG_RELEASE}.apk"
LUCI_DATA="${WORK_DIR}/luci-data"

# --- LuCI staging directory ---
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

# --- Build LuCI package with apk mkpkg ---
# shellcheck disable=SC2086
apk mkpkg \
    --info "name:luci-app-ctrl-modem" \
    --info "version:${APK_VERSION}-r${PKG_RELEASE}" \
    --info "description:CTRL-Modem — LuCI menu integration" \
    --info "arch:noarch" \
    --info "license:proprietary" \
    --info "url:https://ctrl-modem.com" \
    --info "maintainer:agccc <agccc@netsolution.shop>" \
    --info "depends:modem-interface luci-base" \
    --files "${LUCI_DATA}" \
    --output "${OUTPUT_DIR}/${LUCI_APK}" \
    $SIGN_ARGS

echo "  -> ${OUTPUT_DIR}/${LUCI_APK} ($(du -h "${OUTPUT_DIR}/${LUCI_APK}" | cut -f1))"


# ============================================================
# Summary
# ============================================================
echo ""
echo "=== All packages built successfully ==="
echo "Core:  ${OUTPUT_DIR}/${CORE_APK}"
echo "LuCI:  ${OUTPUT_DIR}/${LUCI_APK}"
