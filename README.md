## Overview

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

# create an ImageSetConfig (this uses the example in this repo)
kind: ImageSetConfiguration
apiVersion: alpha1
mirror:
  operators:
  - catalog: "registry.redhat.io/redhat/redhat-operator-index:v4.13"
    packages:
    - name: aws-load-balancer-operator


# execute 
cargo run -- --config imagesetconfig.yaml 
```

