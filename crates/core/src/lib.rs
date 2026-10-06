//! The platform-independent parts of Lumen, shared by the UEFI and BIOS
//! builds: the menu UI and its renderer, fonts and icons, what Lumen knows
//! about operating systems, configuration, and (for booting Linux without
//! GRUB) read-only file systems and Linux boot-entry discovery.

#![no_std]

extern crate alloc;

pub mod config;
pub mod fs;
pub mod gfx;
pub mod icons;
pub mod input;
pub mod linux;
pub mod os;
pub mod text;
pub mod ui;
