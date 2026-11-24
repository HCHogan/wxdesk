# The VM WeChat and wxdesk run in (vm/guest.nix, built with the host's
# nixpkgs), as the service wxdesk-vm, on QEMU's user network. The host
# reaches the guest's ssh, VNC and wxdesk on 127.0.0.1:10022, :15901 and
# :17710. The guest's disk, which holds WeChat's login, is in
# /var/lib/wxdesk-vm; a new one is made if there is none.
#
# A deploy doesn't restart the VM, as booting may ask the phone to confirm
# the login again: a new guest boots when the service is next started. The
# guest runs from the host's store, so the service keeps what it booted
# from the garbage collector.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.wxdesk-vm;
  guest = import "${pkgs.path}/nixos/lib/eval-config.nix" {
    inherit (pkgs.stdenv.hostPlatform) system;
    modules = [ ../vm/guest.nix ];
  };
  run = "${guest.config.system.build.vm}/bin/run-${guest.config.networking.hostName}-vm";
  state = "/var/lib/wxdesk-vm";
  qmp = "/run/wxdesk-vm/qmp.sock";

  # The ACPI power button, which the guest shuts down on; systemd kills
  # QEMU if it hasn't ended in time.
  stop = pkgs.writeShellScript "wxdesk-vm-stop" ''
    [ -n "''${MAINPID:-}" ] || exit 0
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"system_powerdown"}' |
      ${pkgs.socat}/bin/socat -t 2 - UNIX-CONNECT:${qmp} >/dev/null || exit 0
    for _ in $(seq 100); do
      kill -0 "$MAINPID" 2>/dev/null || exit 0
      sleep 1
    done
  '';

  ssh = pkgs.writeShellScriptBin "wxdesk-vm-ssh" ''
    exec ${pkgs.openssh}/bin/ssh -q -i ${state}/ssh-key -p 10022 \
      -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=${state}/known_hosts \
      root@127.0.0.1 "$@"
  '';
in
{
  options.services.wxdesk-vm = {
    enable = lib.mkEnableOption "the VM WeChat and wxdesk run in";
    tokenFile = lib.mkOption {
      type = lib.types.path;
      description = "The token wxdesk's clients send first, given to the guest as it boots.";
    };
    environmentFile = lib.mkOption {
      type = lib.types.path;
      description = ''
        `WXDESK_GROUP=<the group's name>` and `WXDESK_ME=<the bot's display
        name>`, given to the guest as it boots.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.wxdesk-vm = {
      isSystemUser = true;
      group = "wxdesk-vm";
      extraGroups = [ "kvm" ];
    };
    users.groups.wxdesk-vm = { };

    systemd.tmpfiles.rules = [ "L+ /nix/var/nix/gcroots/wxdesk-vm - - - - ${state}/booted" ];

    environment.systemPackages = [ ssh ];

    systemd.services.wxdesk-vm = {
      description = "The VM WeChat and wxdesk run in";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      restartIfChanged = false;
      script = ''
        ln -sfn ${run} ${state}/booted
        export QEMU_OPTS="-qmp unix:${qmp},server=on,wait=off \
          -fw_cfg name=opt/wxdesk/token,file=$CREDENTIALS_DIRECTORY/token \
          -fw_cfg name=opt/wxdesk/env,file=$CREDENTIALS_DIRECTORY/env"
        exec ${run}
      '';
      serviceConfig = {
        User = "wxdesk-vm";
        Group = "wxdesk-vm";
        StateDirectory = "wxdesk-vm";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "wxdesk-vm";
        WorkingDirectory = state;
        LoadCredential = [
          "token:${cfg.tokenFile}"
          "env:${cfg.environmentFile}"
        ];
        ExecStop = stop;
        TimeoutStopSec = 120;
        Restart = "on-failure";
        RestartSec = 10;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
      };
    };
  };
}
