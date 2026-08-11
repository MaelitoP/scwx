{
  description = "Fast SSH, database and port-forward access to Scaleway infrastructure";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        scwx = pkgs.rustPlatform.buildRustPackage {
          pname = "scwx";
          version = cargoToml.package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.installShellFiles ];
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

          postInstall = ''
            installShellCompletion --cmd scwx \
              --zsh <($out/bin/scwx completions zsh) \
              --bash <($out/bin/scwx completions bash) \
              --fish <($out/bin/scwx completions fish)
          '';
        };
      in
      {
        packages = {
          default = scwx;
          scwx = scwx;
        };
      }
    )
    // {
      homeManagerModules.scwx = import ./nix/hm-module.nix self;
      homeManagerModules.default = self.homeManagerModules.scwx;
    };
}
