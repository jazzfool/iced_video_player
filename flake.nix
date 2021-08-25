{
  inputs = {
    flakeCompat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixCargoIntegration = {
      url = "github:yusdacra/nix-cargo-integration";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    inputs.nixCargoIntegration.lib.makeOutputs {
      root = ./.;
      overrides = {
        shell = common: prev: {
          env = prev.env ++ [
            {
              name = "GST_PLUGIN_PATH";
              value = "${common.pkgs.gst_all_1.gstreamer}:${common.pkgs.gst_all_1.gst-plugins-bad}:${common.pkgs.gst_all_1.gst-plugins-ugly}:${common.pkgs.gst_all_1.gst-plugins-good}:${common.pkgs.gst_all_1.gst-plugins-base}";
            }
          ];
        };
      };
    };
}
