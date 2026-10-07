//! `worker` library. T-706 completes the Cloud Run service: the binary
//! (`main.rs`, currently a stub) binds the port and serves the API-INT-2
//! sweep route, which calls [`sweeps::run_sweeps`]. Until then this crate
//! only exports the sweep functions for the tests to drive.

pub mod sweeps;
