{
  description = "Nixy GTK4 greeter for greetd";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      greeter = pkgs.callPackage ./package.nix { };
    in
    {
      packages.${system} = {
        default = greeter;
        nixy-greeter = greeter;
      };
      checks.${system}.greeter = greeter;
      devShells.${system}.default = import ./shell.nix { inherit pkgs; };
      formatter.${system} = pkgs.nixfmt;
    };
}
