# Bonsaimq

Simple database message queue based on [bonsaidb](https://github.com/khonsulabs/bonsaidb).

## Usage

Import the project using:

```toml
bonsaimq = { git = "https://github.com/FlixCoder/bonsaimq" }
```

## Examples

Besides the following simple example, there are more examples in the [examples folder](./examples/)

## Lints

This projects uses a bunch of clippy lints for higher code quality and style.

Install [`cargo-lints`](https://github.com/soramitsu/iroha2-cargo_lints) using `cargo install --git https://github.com/FlixCoder/cargo-lints`. The lints are defined in `lints.toml` and can be checked by running `cargo lints clippy --all-targets --workspace`.
