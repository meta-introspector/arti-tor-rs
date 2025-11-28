<!--
* Use this issue template for reporting a new bug.
-->

### Summary

<!-- Brief overview of what this bug is and what it does. -->


### Steps to reproduce:

1. step 1
2. step 2
3. ...

### What is the current bug behavior?

<!-- Describe the current bug behavior in detail.. -->

### What is the expected behavior?


### Environment

<!--
Please fill in the following information 
-->

- **Arti version:** e.g. 1.7, output of `arti --version`
- **Rust toolchain:** Output of `rustc --version` and `cargo --version`
- **MSRV compliance:** Are you using Rust 1.86 or newer?
- **Install method:** Distribution package, from source (git/tarball), etc.
- **Build profile:** release, release-small, quicktest, or debug
- **Features enabled:** Any relevant features or specific feature flags used
- **Operating system:** Operating system name and version
- **Other environment details:** Any other relevant configuration

### Debugging information

<!--
The following information helps us diagnose the issue. Please include what you can.
-->

- **RUST_LOG output:** If applicable, run with `RUST_LOG=debug` or `RUST_LOG=trace` and include relevant output
- **Backtrace:** If there's a panic, run with `RUST_BACKTRACE=1` and include the backtrace
- **Maintenance checks:** If building from source, please run:
  - `./maint/check_env` - Verify development dependencies
  - `./maint/check_tree` - Check dependency tree
- Relevant logs and/or screenshots:

### (Optional) Possible fixes:

<!-- If you have any ideas for how to fix the bug, please describe them here. If you don't have any ideas, please leave this section blank. -->
- 

/label ~bug
