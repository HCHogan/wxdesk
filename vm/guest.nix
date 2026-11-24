# The VM on tank that wxdesk is tried in: the probe's VM, whose disk holds
# the login, with wxdesk running. tank reaches the guest's ssh, VNC and
# wxdesk on 127.0.0.1:10022, :15901 and :17710.
{
  lib,
  pkgs,
  modulesPath,
  wxdesk,
  wechat,
  ...
}:
let
  probeKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMCV6XgxPbOa6OGO/9x38X98x1xX9XRkLx8F/Z4FNNkn wxprobe";

  python = pkgs.python3.withPackages (ps: [ ps.pygobject3 ]);
  typelibs = lib.makeSearchPath "lib/girepository-1.0" [
    pkgs.at-spi2-core
    pkgs.glib.out
    pkgs.gobject-introspection
  ];

  # Run a command as wx inside the desktop session: `wx <cmd> ...`.
  wx = pkgs.writeShellScriptBin "wx" ''
    set -a; . /run/wx/env; set +a
    if [ "$(id -u)" = 0 ]; then
      exec ${pkgs.util-linux}/bin/runuser -u wx --preserve-environment -- "$@"
    fi
    exec "$@"
  '';

  a11y-dump = pkgs.writeShellScriptBin "a11y-dump" ''
    export GI_TYPELIB_PATH=${typelibs}
    exec ${python}/bin/python3 ${./a11y-dump.py} "$@"
  '';

  shot = pkgs.writeShellScriptBin "shot" ''
    out=''${1:-/tmp/shot.png}
    ${pkgs.imagemagick}/bin/import -display :1 -window root "$out" && echo "$out"
  '';
in
{
  imports = [
    "${modulesPath}/virtualisation/qemu-vm.nix"
    ../nix/module.nix
  ];

  system.stateVersion = "26.05";
  documentation.enable = false;
  networking.hostName = "wxprobe";
  networking.firewall.allowedTCPPorts = [ 5901 ];

  services.wxdesk = {
    enable = true;
    package = wxdesk;
    inherit wechat;
  };

  virtualisation = {
    memorySize = 4096;
    cores = 4;
    diskSize = 16384;
    diskImage = "/var/lib/wxprobe/wxprobe.qcow2";
    graphics = false;
    forwardPorts = [
      {
        from = "host";
        host.address = "127.0.0.1";
        host.port = 10022;
        guest.port = 22;
      }
      {
        from = "host";
        host.address = "127.0.0.1";
        host.port = 15901;
        guest.port = 5901;
      }
      {
        from = "host";
        host.address = "127.0.0.1";
        host.port = 17710;
        guest.port = 7710;
      }
    ];
  };

  services.openssh = {
    enable = true;
    settings.PasswordAuthentication = false;
  };
  users.users.root.openssh.authorizedKeys.keys = [ probeKey ];
  users.users.wx.openssh.authorizedKeys.keys = [ probeKey ];

  environment.systemPackages = [
    wechat
    wx
    a11y-dump
    shot
    python
    pkgs.xdotool
    pkgs.xclip
    pkgs.xprop
    pkgs.xwininfo
    pkgs.imagemagick
    pkgs.netcat-openbsd
  ];
}
