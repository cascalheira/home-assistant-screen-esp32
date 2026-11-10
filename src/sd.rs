//! microSD card on SPI2 (CS=10, MOSI=11, SCK=12, MISO=13), mounted as FAT at
//! `/sdcard`. Once mounted it is used through `std::fs`.
//!
//! The card is probed once at boot; a card inserted later needs a restart.

use std::ffi::CString;

use anyhow::Result;
use esp_idf_svc::fs::fatfs::Fatfs;
use esp_idf_svc::hal::gpio::{AnyIOPin, InputPin, OutputPin};
use esp_idf_svc::hal::sd::{spi::SdSpiHostDriver, SdCardConfiguration, SdCardDriver};
use esp_idf_svc::hal::spi::{config::DriverConfig, Dma, SpiAnyPins, SpiDriver};
use esp_idf_svc::io::vfs::MountedFatfs;
use esp_idf_svc::sys;

pub const MOUNT_POINT: &str = "/sdcard";

type Card = SdCardDriver<SdSpiHostDriver<'static, SpiDriver<'static>>>;

pub struct SdCard {
    _mounted: MountedFatfs<Fatfs<Card>>,
}

impl SdCard {
    /// Initializes the card and mounts it. Fails when no card is inserted
    /// or it isn't FAT formatted.
    pub fn mount(
        spi: impl SpiAnyPins + 'static,
        sck: impl OutputPin + 'static,
        mosi: impl OutputPin + 'static,
        miso: impl InputPin + 'static,
        cs: impl OutputPin + 'static,
    ) -> Result<Self> {
        let spi = SpiDriver::new(
            spi,
            sck,
            mosi,
            Some(miso),
            &DriverConfig::default().dma(Dma::Auto(4096)),
        )?;
        let host = SdSpiHostDriver::new(
            spi,
            Some(cs),
            AnyIOPin::none(),
            AnyIOPin::none(),
            AnyIOPin::none(),
            None,
        )?;
        let card = SdCardDriver::new_spi(host, &SdCardConfiguration::new())?;
        let mounted = MountedFatfs::mount(Fatfs::new_sdcard(0, card)?, MOUNT_POINT, 4)?;
        let card = Self { _mounted: mounted };
        if let Some((total, free)) = card.usage() {
            log::info!(
                "SD card mounted at {MOUNT_POINT}: {} total, {} free",
                size(total),
                size(free)
            );
        }
        Ok(card)
    }

    /// (total, free) bytes.
    pub fn usage(&self) -> Option<(u64, u64)> {
        let path = CString::new(MOUNT_POINT).ok()?;
        let (mut total, mut free) = (0u64, 0u64);
        let err = unsafe { sys::esp_vfs_fat_info(path.as_ptr(), &mut total, &mut free) };
        (err == sys::ESP_OK).then_some((total, free))
    }

    /// Capacity of the whole card in bytes (all partitions, not just ours).
    pub fn card_size(&self) -> Option<u64> {
        let mut sectors: sys::LBA_t = 0;
        let mut sector_size: sys::WORD = 0;
        unsafe {
            let a = sys::ff_disk_ioctl(
                0,
                sys::GET_SECTOR_COUNT as u8,
                (&mut sectors as *mut sys::LBA_t).cast(),
            );
            let b = sys::ff_disk_ioctl(
                0,
                sys::GET_SECTOR_SIZE as u8,
                (&mut sector_size as *mut sys::WORD).cast(),
            );
            (a == 0 && b == 0).then_some(sectors as u64 * sector_size as u64)
        }
    }

    /// Erases the whole card: replaces the partition table with a single
    /// partition spanning the card and formats it as FAT32 (no volume label:
    /// ESP-IDF's FatFs is built without label support). Everything on the
    /// card is lost. The card is left
    /// unmounted; restart to mount it again.
    pub fn format_whole_card(&self) -> Result<()> {
        const WORK: usize = 4096;
        let drive = c"0:";
        let mut work = vec![0u8; WORK];
        unsafe {
            let res = sys::f_mount(core::ptr::null_mut(), drive.as_ptr(), 0);
            if res != sys::FRESULT_FR_OK {
                anyhow::bail!("unmount failed ({res})");
            }
            // One partition taking 100% of the card.
            let plist: [sys::LBA_t; 4] = [100, 0, 0, 0];
            let res = sys::f_fdisk(0, plist.as_ptr(), work.as_mut_ptr().cast());
            if res != sys::FRESULT_FR_OK {
                anyhow::bail!("partitioning failed ({res})");
            }
            log::warn!("SD card repartitioned");
            let opt = sys::MKFS_PARM {
                fmt: sys::FM_ANY as u8, // FAT32 for cards over 2 GB (exFAT isn't built in)
                n_fat: 2,
                align: 0,
                n_root: 0,
                au_size: 0, // let FatFs pick the cluster size for the card size
            };
            let res = sys::f_mkfs(drive.as_ptr(), &opt, work.as_mut_ptr().cast(), WORK as u32);
            if res != sys::FRESULT_FR_OK {
                anyhow::bail!("formatting failed ({res})");
            }
            log::warn!("SD card formatted");
        }
        Ok(())
    }

    /// One line per entry in the card's root directory, for the console.
    pub fn listing(&self) -> String {
        match std::fs::read_dir(MOUNT_POINT) {
            Ok(entries) => {
                let mut lines: Vec<String> = entries
                    .flatten()
                    .map(|e| {
                        let meta = e.metadata().ok();
                        let kind = if meta.as_ref().is_some_and(|m| m.is_dir()) {
                            "dir "
                        } else {
                            "file"
                        };
                        let len = meta.map(|m| m.len()).unwrap_or(0);
                        format!("{kind} {:>10} {}", len, e.file_name().to_string_lossy())
                    })
                    .collect();
                lines.sort();
                if lines.is_empty() {
                    "(empty)".into()
                } else {
                    lines.join("\n")
                }
            }
            Err(e) => format!("can't read {MOUNT_POINT}: {e}"),
        }
    }
}

/// Human-readable size: "29.7 GB", "512 MB".
pub fn size(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}
