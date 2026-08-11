self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.scwx;
  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.scwx = {
    enable = lib.mkEnableOption "scwx";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.default;
      description = "The scwx package to install.";
    };

    settings = lib.mkOption {
      type = tomlFormat.type;
      default = { };
      example = lib.literalExpression ''
        {
          ssh.key = "~/.local/share/ssh/id_ed25519_scaleway";
          naming.strip_prefixes = [ "platform-ingestor-" ];
          db.secret_project_id = "00000000-0000-0000-0000-000000000000";
        }
      '';
      description = "Contents of ~/.config/scwx/config.toml.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."scwx/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "scwx-config.toml" cfg.settings;
    };
  };
}
