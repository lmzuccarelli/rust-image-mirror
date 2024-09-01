## Overview

This document outlines how to execute tests, obtain code coverage and debug the application

### Testing & Debugging

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

**NB** a Makefile is included in the repo so to execute test and code coverage execute the following command

```
make test && make cover
```



To Debug

execute the correct make

```
make build-debug

# launch rust-gdb

rust-gdb --args target/debug/image-mirror --config imagesetconfig.yaml  --loglevel info

# set breakpoint
b src/release/collector.rs:461

# execute run
r

# step
n

# print
p img.name
```

### Coverage Overview

As of the time of writing (01/09/2024) the coverage is shown in the diagram below

![Cover](assets/coverage-overview.png)

