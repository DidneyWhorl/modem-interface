-- CTRL-Modem LuCI controller for OpenWRT 19.07-21.02 (Lua-based LuCI)
-- On 22.03+ this file is ignored — the JSON menu in /usr/share/luci/menu.d/ takes priority.
module("luci.controller.ctrl-modem", package.seeall)

function index()
    entry({"admin", "network", "ctrl_modem"}, template("ctrl_modem_redirect"), _("CTRL-Modem"), 90)
        .dependent = false
end
