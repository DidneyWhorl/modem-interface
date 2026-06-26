include $(TOPDIR)/rules.mk

PKG_NAME:=modem-interface
PKG_VERSION:=1.5.0
PKG_RELEASE:=1

PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)
PKG_SOURCE:=$(PKG_NAME)_$(PKG_VERSION)-$(PKG_RELEASE)_aarch64_cortex-a53.ipk
PKG_SOURCE_URL:=https://packages.ctrl-modem.com/stable/feed/aarch64_cortex-a53/
PKG_HASH:=00744eaaf2c1d3ef603c437c55709e30b9b66460666a05e922bf00d629b0f1f0

PKG_MAINTAINER:=agccc <agccc@netsolution.shop>
PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE

include $(INCLUDE_DIR)/package.mk

# --- Package 1: modem-interface (core) ---

define Package/modem-interface
  SECTION:=net
  CATEGORY:=Network
  TITLE:=CTRL-Modem — cellular modem management interface
  URL:=https://github.com/DidneyWhorl/modem-interface
  DEPENDS:=+kmod-usb-serial +kmod-usb-serial-option +kmod-usb-serial-wwan +ca-bundle
endef

define Package/modem-interface/description
  A standalone web interface for managing cellular modems on OpenWRT routers.
  Part of the CTRL-Modem suite. Supports 4G/5G modems with real-time signal
  monitoring, connection management, and multi-WAN failover.
endef

define Package/modem-interface/conffiles
/etc/config/modem-interface
/etc/modem-interface/config.toml
endef

# --- Package 2: luci-app-ctrl-modem (LuCI integration) ---

define Package/luci-app-ctrl-modem
  SECTION:=luci
  CATEGORY:=LuCI
  SUBCATEGORY:=3. Applications
  TITLE:=CTRL-Modem — LuCI menu integration
  URL:=https://github.com/DidneyWhorl/modem-interface
  DEPENDS:=+modem-interface +luci-base
  PKGARCH:=all
endef

define Package/luci-app-ctrl-modem/description
  Adds a CTRL-Modem entry to the LuCI web interface under Network.
  Part of the CTRL-Modem suite. Supports LuCI 19.07+ (Lua) and 22.03+ (JSON).
  Clicking the menu entry redirects to the CTRL-Modem web interface.
endef

# --- Build (shared — pre-built artifacts from CI) ---

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
	# Note: Binary and frontend are pre-built via CI (cargo-zigbuild + npm)
	# This Makefile packages pre-compiled artifacts
endef

define Build/Compile
	# No compilation needed - using pre-built binaries from CI
	# Backend: Cross-compiled with cargo-zigbuild for aarch64-unknown-linux-musl
	# Frontend: Built with Vite + React, embedded in binary via rust-embed
endef

# --- Core package install ---

define Package/modem-interface/install
	# Install binary
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/usr/bin/modem-interface $(1)/usr/bin/

	# Install init script
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/etc/init.d/modem-interface $(1)/etc/init.d/

	# Install UCI config
	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) $(PKG_BUILD_DIR)/etc/config/modem-interface $(1)/etc/config/

	# Install application config
	$(INSTALL_DIR) $(1)/etc/modem-interface
	$(INSTALL_CONF) $(PKG_BUILD_DIR)/etc/modem-interface/config.toml $(1)/etc/modem-interface/

	# Install update script
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/modem-interface-update $(1)/usr/bin/

	# Install cron job for auto-updates
	$(INSTALL_DIR) $(1)/etc/cron.d
	$(INSTALL_DATA) ./files/etc/cron.d/modem-interface-update $(1)/etc/cron.d/
endef

define Package/modem-interface/postinst
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	echo "Enabling modem-interface service..."
	/etc/init.d/modem-interface enable
	/etc/init.d/modem-interface start
	# Restart cron to pick up new cron.d entry for auto-updates
	/etc/init.d/cron restart 2>/dev/null || true
	ROUTER_IP=$$(uci -q get network.lan.ipaddr || echo "192.168.1.1")
	echo ""
	echo "CTRL-Modem is running."
	echo "  Access: https://$${ROUTER_IP}:8443/ctrl-modem/"
	if opkg status luci-app-ctrl-modem 2>/dev/null | grep -q "^Status:.*installed"; then
		echo "  LuCI menu: Network -> CTRL-Modem"
	fi
	echo ""
}
exit 0
endef

define Package/modem-interface/prerm
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	echo "Stopping modem-interface service..."
	/etc/init.d/modem-interface stop
	/etc/init.d/modem-interface disable
}
exit 0
endef

# --- LuCI package install ---

define Package/luci-app-ctrl-modem/install
	# JSON menu entry (LuCI 22.03+)
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./files/usr/share/luci/menu.d/luci-app-ctrl-modem.json $(1)/usr/share/luci/menu.d/

	# JS redirect view (LuCI 22.03+)
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/network
	$(INSTALL_DATA) ./files/www/luci-static/resources/view/network/ctrl-modem.js $(1)/www/luci-static/resources/view/network/

	# Lua controller (LuCI 19.07-21.02)
	$(INSTALL_DIR) $(1)/usr/lib/lua/luci/controller
	$(INSTALL_DATA) ./files/usr/lib/lua/luci/controller/ctrl-modem.lua $(1)/usr/lib/lua/luci/controller/

	# Lua redirect template (LuCI 19.07-21.02)
	$(INSTALL_DIR) $(1)/usr/lib/lua/luci/view
	$(INSTALL_DATA) ./files/usr/lib/lua/luci/view/ctrl_modem_redirect.htm $(1)/usr/lib/lua/luci/view/
endef

$(eval $(call BuildPackage,modem-interface))
$(eval $(call BuildPackage,luci-app-ctrl-modem))
