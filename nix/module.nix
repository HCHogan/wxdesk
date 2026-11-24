# WeChat in an X display of its own, as the user wx, with wxdesk driving it.
#
# The display is Xvnc :1 (1280x800) with openbox, in a D-Bus session of its
# own with the accessibility bus. Log WeChat in through VNC once (the
# password is in /var/lib/wxvnc/password); the login lasts in /home/wx.
# wxdesk then serves on `port` and wants the token in `tokenFile`.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.wxdesk;
  envFile = "/run/wx/env";

  inner = pkgs.writeShellScript "wx-session-inner" ''
    set -u
    {
      echo "DISPLAY=$DISPLAY"
      echo "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
      echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"
      echo "HOME=$HOME"
      echo "LANG=$LANG"
    } > ${envFile}
    ${pkgs.at-spi2-core}/libexec/at-spi-bus-launcher --launch-immediately &
    ${pkgs.openbox}/bin/openbox &
    ${pkgs.xset}/bin/xset s off -dpms s noblank
    sleep 2
    while true; do
      ${lib.getExe cfg.wechat} --no-sandbox --disable-gpu
      echo "wechat exited with $?; starting it again in 5 s"
      sleep 5
    done
  '';

  session = pkgs.writeShellScript "wx-session" ''
    set -u
    ${pkgs.tigervnc}/bin/Xvnc :1 -geometry 1280x800 -depth 24 \
      -rfbport 5901 -SecurityTypes VncAuth -PasswordFile /var/lib/wxvnc/passwd \
      -AlwaysShared -desktop wxdesk &
    for _ in $(seq 100); do [ -e /tmp/.X11-unix/X1 ] && break; sleep 0.1; done
    export DISPLAY=:1
    exec ${pkgs.dbus}/bin/dbus-run-session \
      --dbus-daemon=${pkgs.dbus}/bin/dbus-daemon \
      --config-file=/etc/dbus-1/session.conf -- ${inner}
  '';
in
{
  options.services.wxdesk = {
    enable = lib.mkEnableOption "WeChat driven by wxdesk";
    package = lib.mkOption {
      type = lib.types.package;
      description = "wxdesk.";
    };
    wechat = lib.mkOption {
      type = lib.types.package;
      description = "WeChat for Linux.";
    };
    port = lib.mkOption {
      type = lib.types.port;
      default = 7710;
    };
    tokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        The token clients send first, readable by wx; null makes one in
        /var/lib/wxdesk/token.
      '';
    };
    environmentFile = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/wxdesk/env";
      description = ''
        `WXDESK_GROUP=<the group's name>`, and `WXDESK_ME=<the bot's
        display name>` if known: kept out of the store and the repository.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    nixpkgs.config.allowUnfreePredicate = pkg: lib.getName pkg == "wechat";

    users.users.wx = {
      isNormalUser = true;
      uid = 1000;
    };

    hardware.graphics.enable = true;
    services.gnome.at-spi2-core.enable = true;
    fonts.packages = [
      pkgs.noto-fonts-cjk-sans
      pkgs.noto-fonts-color-emoji
    ];
    time.timeZone = lib.mkDefault "Asia/Shanghai";
    i18n.supportedLocales = [
      "en_US.UTF-8/UTF-8"
      "zh_CN.UTF-8/UTF-8"
    ];

    # An 8-character VNC password and the token, each made once.
    systemd.services.wxdesk-secrets = {
      wantedBy = [ "multi-user.target" ];
      before = [
        "wx-desktop.service"
        "wxdesk.service"
      ];
      serviceConfig.Type = "oneshot";
      script = ''
        install -d -m 0750 -o root -g users /var/lib/wxvnc
        if [ ! -e /var/lib/wxvnc/passwd ]; then
          pw=$(tr -dc A-Za-z0-9 < /dev/urandom | head -c 8)
          umask 077
          printf '%s\n' "$pw" > /var/lib/wxvnc/password
          printf '%s\n' "$pw" | ${pkgs.tigervnc}/bin/vncpasswd -f > /var/lib/wxvnc/passwd
          chown wx /var/lib/wxvnc/passwd
        fi
        install -d -m 0750 -o wx -g users /var/lib/wxdesk
      ''
      + lib.optionalString (cfg.tokenFile == null) ''
        if [ ! -e /var/lib/wxdesk/token ]; then
          umask 077
          tr -dc A-Za-z0-9 < /dev/urandom | head -c 32 > /var/lib/wxdesk/token
          chown wx /var/lib/wxdesk/token
        fi
      '';
    };

    systemd.services.wx-desktop = {
      wantedBy = [ "multi-user.target" ];
      after = [
        "network-online.target"
        "wxdesk-secrets.service"
      ];
      wants = [ "network-online.target" ];
      environment = {
        HOME = "/home/wx";
        XDG_RUNTIME_DIR = "/run/wx";
        LANG = "zh_CN.UTF-8";
        QT_ACCESSIBILITY = "1";
        QT_LINUX_ACCESSIBILITY_ALWAYS_ON = "1";
      };
      serviceConfig = {
        User = "wx";
        RuntimeDirectory = "wx";
        RuntimeDirectoryMode = "0755";
        ExecStart = session;
        Restart = "always";
        RestartSec = 5;
      };
    };

    systemd.services.wxdesk = {
      wantedBy = [ "multi-user.target" ];
      after = [ "wx-desktop.service" ];
      partOf = [ "wx-desktop.service" ];
      path = [
        pkgs.xdotool
        pkgs.xclip
      ];
      environment = {
        WXDESK_LISTEN = "0.0.0.0:${toString cfg.port}";
        WXDESK_TOKEN_FILE = if cfg.tokenFile == null then "/var/lib/wxdesk/token" else cfg.tokenFile;
      };
      script = ''
        while [ ! -s ${envFile} ]; do sleep 1; done
        set -a; . ${envFile}; set +a
        exec ${lib.getExe cfg.package}
      '';
      serviceConfig = {
        User = "wx";
        EnvironmentFile = cfg.environmentFile;
        Restart = "always";
        RestartSec = 5;
      };
    };

    networking.firewall.allowedTCPPorts = [ cfg.port ];
  };
}
