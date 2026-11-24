{
  description = "wxdesk: WeChat for Linux, read through AT-SPI and driven with XTEST";

  # The nixpkgs tank runs (nixos-26.05), with its hash, so tank takes it
  # from its own store instead of fetching it.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/0dc498bfe49baeb18b0f9891e800b53bb2603193?narHash=sha256-s/0jq8usZtLVOolYBwmyUmd3JsfAu7w3FU%2Ba0dgMqSY%3D";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        config.allowUnfreePredicate = pkg: nixpkgs.lib.getName pkg == "wechat";
      };
    in
    {
      packages.${system} = {
        wxdesk = pkgs.callPackage ./nix/wxdesk.nix { };
        wechat = pkgs.callPackage ./nix/wechat.nix { };
        default = self.packages.${system}.wxdesk;
      };

      # WeChat and wxdesk, on a NixOS machine.
      nixosModules.default = ./nix/module.nix;
      # The VM they run in, on the host that runs it.
      nixosModules.vm = ./nix/vm.nix;

      nixosConfigurations.wxdesk-vm = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [ ./vm/guest.nix ];
      };
    };
}
