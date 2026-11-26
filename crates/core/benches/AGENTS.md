# Core benchmarks

Benchmarks here can also run on a Kobo with `cadmuscli kobo bench`, away from
the source tree. A benchmark that is meant to run there must:

- not read fixtures through `env!("CARGO_MANIFEST_DIR")` or any other path
  fixed at compile time; the source tree does not exist on the device.
- generate or download fixtures under the directory in the `CADMUS_BENCH_DIR`
  environment variable, and check the free space there before writing a large
  file.
- keep scratch data out of `/tmp`, which is RAM on a Kobo.
