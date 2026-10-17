{
  description = "Levelator NG: a Rust port of The Levelator, with noise reduction and more formats";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # winit/glutin load these at runtime with dlopen, so on NixOS they must
      # be on the binary's RUNPATH (they are not in any standard location).
      # The X11 libraries moved out of `xorg` in newer nixpkgs; fall back to
      # the old names so older nixpkgs still evaluate without warnings.
      guiRuntimeLibs =
        pkgs: with pkgs; [
          wayland
          libxkbcommon
          libGL
          (pkgs.libx11 or pkgs.xorg.libX11)
          (pkgs.libxcursor or pkgs.xorg.libXcursor)
          (pkgs.libxi or pkgs.xorg.libXi)
          (pkgs.libxrandr or pkgs.xorg.libXrandr)
        ];
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib stdenv;
          cargoToml = lib.importTOML ./Cargo.toml;
        in
        {
          default = self.packages.${stdenv.hostPlatform.system}.levelator;

          levelator = pkgs.rustPlatform.buildRustPackage {
            pname = "levelator";
            version = cargoToml.workspace.package.version;
            src = lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;

            # LAME's bundled ./configure and the libvorbis C sources.
            nativeBuildInputs = [ pkgs.gnumake ];

            postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
              patchelf --add-rpath ${lib.makeLibraryPath (guiRuntimeLibs pkgs)} $out/bin/levelator
            '';

            meta = {
              description = "Level the loudness of spoken-word recordings (Levelator 2.1.2 engine)";
              homepage = "https://github.com/jack3898/levelator-ng";
              license = lib.licenses.mit;
              mainProgram = "levelator";
              platforms = systems;
            };
          };
        }
      );

      apps = forAllSystems (
        pkgs:
        let
          pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.levelator;
        in
        {
          default = {
            type = "app";
            program = "${pkg}/bin/levelator";
          };
          level = {
            type = "app";
            program = "${pkg}/bin/level";
          };
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            gnumake
          ];
          LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
            pkgs.lib.makeLibraryPath (guiRuntimeLibs pkgs)
          );
        };
      });
    };
}
