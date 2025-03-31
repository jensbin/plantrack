{ lib
, rustPlatform
}:
let
  fs = lib.fileset;
  sourceFiles = fs.unions [
    ./Cargo.toml
    ./Cargo.lock
    ./src/main.rs
  ];
in
rustPlatform.buildRustPackage {
  pname = "plantrack";
  version = "0.1.6";

  src = fs.toSource {
    root = ./.;
    fileset = sourceFiles;
  };

  # cargoHash = lib.fakeHash;
  cargoLock = {                                                                                                                                                     
    lockFile = ./Cargo.lock;                                                                                                                                        
  };                       

  meta = with lib; {
    description = "Plan and track of multi activities";
    license = licenses.mit;
  };
}
