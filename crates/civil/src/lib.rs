//! Civil-ING: the civil-engineering and surveying module of CADCraft.
//!
//! Everything here is plain computation with no drawing or UI types beyond reading points out of
//! a [`cadcraft_doc::Drawing`], so it is tested on its own and the engine commands stay thin:
//!
//! - [`points`]: survey points from delimited text (CSV, TXT, tab or space separated), from XLSX
//!   ([`xlsx`]) and from the objects of a DXF or DWG drawing.
//! - [`radiation`]: points computed from total-station observations (station, angles, distances).
//! - [`tin`]: a Delaunay triangulated irregular network over the points, with elevation lookup.
//! - [`contour`]: contour polylines at a chosen interval.
//! - [`alignment`]: a road or channel axis with stations, and the ground profile along it.
//! - [`ai`]: the client for the AI assistant (Anthropic and Gemini).
//!
//! The module is original work, written from the public description of the surveying tasks (point
//! files, triangulation, contouring, stationing); it does not use any other product's code.

pub mod ai;
pub mod alignment;
pub mod contour;
pub mod earthwork;
pub mod parcel;
pub mod platform;
pub mod plugins;
pub mod points;
pub mod procedures;
pub mod radiation;
pub mod selfmod;
pub mod source;
pub mod tin;
pub mod xlsx;
