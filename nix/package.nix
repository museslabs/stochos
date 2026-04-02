{
  lib,
  craneLib,
  stdenv,
  wayland,
  libx11,
  ...
}: let
  unfilteredRoot = ../.;

  libs = [
    wayland
    libx11
  ];
  libsPath = lib.makeLibraryPath libs;

  commonArgs = {
    src = craneLib.cleanCargoSource unfilteredRoot;

    strictDeps = true;

    buildInputs = libs;
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
  craneLib.buildPackage (commonArgs
    // {
      inherit cargoArtifacts;

      postFixup = ''
        patchelf $out/bin/stochos --add-rpath ${libsPath}
      '';

      passthru = {
        runtimeLibsPath = libsPath;
      };

      meta = {
        description = "Keyboard-driven mouse control overlay for Wayland and X11";
        homepage = "https://github.com/museslabs/stochos";
        license = lib.licenses.gpl3Only;
        maintainers = with lib.maintainers; [
          tukanoidd
          ploMP4
        ];
        mainProgram = "stochos";
      };
    })
