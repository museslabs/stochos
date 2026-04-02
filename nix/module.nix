packages: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.stochos;
  tomlFormat = pkgs.formats.toml {};
in {
  options.programs.stochos = {
    enable = lib.mkEnableOption "stochos";
    package = lib.mkOption {
      type = lib.types.package;
      default = packages.${pkgs.stdenv.hostPlatform.system}.stochos;
      description = "Package including stochos binary (e.g. stochos.packages.\${pkgs.stdenv.hostPlatform.system}.[default/stochos])";
    };
    settings = lib.mkOption {
      inherit (tomlFormat) type;
      default = {};
      example = {
        grid = {
          hints = ["a" "s" "d" "f" "j" "k" "l" ";" "g" "h" "q" "w" "e" "r" "t" "y" "u" "i" "o" "p"];
          sub_hints = ["a" "s" "d" "f" "j" "k" "l" ";" "g" "h" "q" "w" "e" "r" "t" "y" "u" "i" "o" "p" "z" "x" "c" "v" "b"];
          sub_cols = 5;
        };

        keys = {
          click = "space";
          double_click = "enter";
          close = "escape";
          undo = "backspace";
          right_click = "delete";
          scroll_up = "up";
          scroll_down = "down";
          scroll_left = "left";
          scroll_right = "right";
          macro_menu = "tab";
          macro_record = "`";
        };
      };
      description = ''
        Configuration settings for stochos. Find more information at:
        <https://github.com/museslabs/stochos?tab=readme-ov-file#configuration>
      '';
    };
  };
  config = lib.mkIf cfg.enable {
    home.packages = lib.mkIf (cfg.package != null) [cfg.package];
    xdg.configFile."stochos/config.toml" = lib.mkIf (cfg.settings != {}) {
      source = tomlFormat.generate "config.toml" cfg.settings;
    };
  };
}
