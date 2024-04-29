## Overview

![Badges](assets/flat.svg)

This is a simple POC that mirrors ocp/okd release, operator and additional images in dockerv2 format (from a registry) to disk 
and from disk to mirror

## POC 

This is still a WIP. It will use the head of the defaultChannel (for operators) 
and specific release version only (for platform/release images)

I used a simple approach - Occam's razor

- A scientific and philosophical rule that entities should not be multiplied unnecessarily (KISS)
- Worked with a v2 images for the POC
- only operators have been included for now
- release and additional images are not implemented yet

## Usage

Clone this repo

Ensure that you have the correct permissions set in the $XDG_RUNTIME_DIR/containers/auth.json file

Execute the following to copy to local disk 

```bash
mkdir -p working-dir/rhopi/blobs/sha256
cargo build 

# use the catalog and release introspection tools to create a merged ImageSetConfig (this uses the example in this repo)
# refer to https://github.com/lmzuccarelli/rust-release-introspection-tool and https://github.com/lmzuccarelli/rust-catalog-introspection-tool for more details
kind: ImageSetConfiguration
apiVersion: mirror.openshift/v3alpha1
mirror:
  release: 
  - version: "4.14.16"
    image: "quay.io/openshift-release-dev/ocp-release:4.14.6-x86_64"
  operators:
  - catalog: "registry.redhat.io/redhat/redhat-operator-index:v4.14"
    packages:
    - name: aws-load-balancer-operator
      bundles: 
      - name: "aws-load-balancer-operator.v1.1.0"  

# execute 
cargo run -- --config imagesetconfig.yaml 
```

## Testing

Ensure grcov and  llvm tools-preview are installed

```
cargo install grcov 

rustup component add llvm-tools-preview

```

execute the tests

```
# add the -- --nocapture or --show-ouput flags to see println! statements
$ CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='cargo-test-%p-%m.profraw' cargo test

# for individual tests
$ CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='cargo-test-%p-%m.profraw' cargo test create_diff_tar_pass -- --show-output
```

check the code coverage

```
$ grcov . --binary-path ./target/debug/deps/ -s . -t html --branch --ignore-not-existing --ignore '../*' --ignore "/*" --ignore "src/main.rs" -o target/coverage/html

```

### Coverage Overview

![Cover](assets/coverage-overview.png)

