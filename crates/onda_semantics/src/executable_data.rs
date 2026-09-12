//! Fixed storage and symbolic data selections shared by executable owners.
use crate::*;

mod bindings;
mod storage;
mod views;
pub(crate) use bindings::*;
pub(crate) use storage::*;
pub(crate) use views::{CapturedViews, ViewScope};
