#[cfg(windows)]
pub mod card;
pub mod geometry;
#[cfg(windows)]
pub mod install;
pub mod layer;
pub mod simulation;
#[cfg(windows)]
pub mod update;
pub mod version;

#[cfg(test)]
mod tests;
