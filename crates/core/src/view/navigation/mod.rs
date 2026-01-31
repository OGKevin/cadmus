//! Generic navigation UI components.
//!
//! This module provides a reusable stacked navigation bar used for traversing
//! hierarchical domains (directories today, tags/artists/series in the future).
//!
//! The implementation is split into:
//! - [`stack_navigation_bar`]: the generic container view and its core traits
//! - `providers/*`: domain-specific adapters that plug existing views into the
//!   generic container.

pub mod stack_navigation_bar;

pub use stack_navigation_bar::StackNavigationBar;

pub mod providers {
    //! Domain-specific providers for [`super::stack_navigation_bar`].

    pub mod directory;
}
