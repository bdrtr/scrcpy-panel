fn main() {
    // Access the raw FFmpeg sys bindings through ffmpeg_next
    unsafe {
        let hw_type = ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA;
        println!("D3D11VA type = {:?}", hw_type as i32);

        let dxva2 = ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2;
        println!("DXVA2 type = {:?}", dxva2 as i32);

        let cuda = ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA;
        println!("CUDA type = {:?}", cuda as i32);

        // Check if hw device ctx create exists
        let _ = ffmpeg_next::ffi::av_hwdevice_ctx_create as *const ();
        let _ = ffmpeg_next::ffi::av_hwframe_transfer_data as *const ();
        let _ = ffmpeg_next::ffi::avcodec_get_hw_config as *const ();
        println!("All hw APIs available!");
    }
}
