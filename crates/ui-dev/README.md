# UI Dev

A lightweight development environment for creating and testing individual Plato UI components in isolation.

## Overview

`ui-dev` is a specialized emulator crate that spins up a single-view SDL2 window, making it easy to develop, test, and iterate on UI components without the overhead of the full Plato application.

## Getting Started

### Initial Setup

1. Copy the template to create your view implementation:

```bash
cp crates/ui-dev/src/view.rs.template crates/ui-dev/src/view.rs
```

2. Edit `crates/ui-dev/src/view.rs` to render your component

3. Run the emulator:

```bash
cargo run -p ui-dev
```
