//! Starting, stopping and waiting on the file a session writes.
//!
//! Recording outlives the thing that asked for it: the panel can turn it on
//! and off mid-session, and the muxer's trailer is written after the last
//! packet, so stopping means waiting for a file rather than dropping a handle.

use super::*;

impl Session {
    /// Start the thread that writes the recording, once the streams have said
    /// what they are.
    ///
    /// This lived inside `start_video`, and so did not happen at all without
    /// one. `--no-video --record=out.mkv` therefore made a recorder, teed every
    /// audio packet into it from `run_audio_pipeline`, and never started the
    /// thread that drains it: the packets banked up in a queue nobody read and
    /// the file was never created — asked for eight seconds of audio, the
    /// client wrote nothing and said nothing about it. Audio-only recording is
    /// what upstream's `--no-video --record=file.opus` is.
    pub(super) fn spawn_recorder(&mut self, opts: &Options) {
        let Some(path) = opts.record.clone() else {
            return;
        };
        let thread = {
            let guard = self.recorder.read().expect("recorder lock");
            let Some(rec) = guard.as_ref() else {
                return;
            };
            rec.spawn(
                path.clone(),
                opts.record_format.clone(),
                opts.video_enabled(),
                // What the device gave, not what was asked for: a server that
                // declines audio would otherwise leave this waiting for ever
                // with the whole stream banked up behind it.
                self.audio_codec_id,
                opts.record_rotation(),
            )
        };
        *self.recorder_thread.lock().expect("recorder thread lock") = Some(thread);
        match opts.record_format {
            Some(ref format) => log::info!("Recording to: {} ({})", path, format),
            None => log::info!("Recording to: {}", path),
        }
    }

    /// Is a recording running right now?
    pub fn is_recording(&self) -> bool {
        self.recorder.read().expect("recorder lock").is_some()
    }

    /// Start recording a session that is already running.
    ///
    /// The demux threads read the recorder out of a shared slot on every
    /// packet, so installing one here is enough — but the stream is mid-GOP,
    /// and a file that starts on a delta frame is unplayable until the next
    /// keyframe. `ControlMsg::ResetVideo` asks the device for a fresh config
    /// and keyframe, which is what makes a mid-session recording start clean.
    /// `controller` comes from the caller because the host takes the session's
    /// own control channel when it mounts the mirror.
    pub fn start_recording(
        &self,
        path: &str,
        format: Option<&str>,
        controller: Option<&Controller>,
    ) -> Result<()> {
        if self.is_recording() {
            anyhow::bail!("Already recording");
        }
        let video_codec = self
            .video_codec
            .clone()
            .context("Cannot record without a video stream")?;

        let recorder = Recorder::new();
        recorder.set_video_codec(video_codec);
        let has_audio = self.audio_codec_id.is_some();
        if let Some(codec_id) = self.audio_codec_id {
            recorder.set_audio_codec(codec_id, true);
        }
        let thread = recorder.spawn(
            path.to_string(),
            format.map(str::to_string),
            true,
            self.audio_codec_id,
            self.record_rotation,
        );
        *self.recorder_thread.lock().expect("recorder thread lock") = Some(thread);

        // Seed the queues with the config packets from the top of the stream,
        // before the recorder is visible to the demuxers, so they are the first
        // thing it reads. The audio one is the reason: OpusHead is sent once and
        // will not come again.
        if let Some(config) = self.video_config.lock().expect("config lock").clone() {
            recorder.push_video(RecPacket {
                data: config,
                pts: i64::MIN,
                is_key: false,
            });
        }
        if has_audio {
            if let Some(config) = self.audio_config.lock().expect("config lock").clone() {
                recorder.push_audio(RecPacket {
                    data: config,
                    pts: i64::MIN,
                    is_key: false,
                });
            }
        }

        *self.recorder.write().expect("recorder lock") = Some(recorder);

        if let Some(controller) = controller.or(self.controller.as_ref()) {
            controller.push_msg(ControlMsg::ResetVideo);
        } else {
            log::warn!(
                "Recording started without a control channel, so the file begins \
                 at the next keyframe the device happens to send"
            );
        }

        log::info!("Recording started: {}", path);
        Ok(())
    }

    /// Stop a recording without ending the session, and wait for the file.
    ///
    /// Waiting is the point: the trailer is written after the last packet, and
    /// an mp4 without one does not open. This used to return the moment the
    /// recorder was told to stop, so a caller said "recording stopped" over a
    /// file that was not yet one.
    pub fn stop_recording(&self) -> bool {
        // Taken out from under the lock on its own line, so the write guard is
        // gone before the wait begins. Held across it — which is what a `match`
        // on the guard does, since the temporary lives to the end of the
        // statement — the demuxers block on their next packet, because they
        // take this same lock for every one, and the mirror stops for as long
        // as the file takes to finish.
        let recorder = self.recorder.write().expect("recorder lock").take();
        match recorder {
            Some(recorder) => {
                recorder.stop();
                self.wait_for_the_file();
                log::info!("Recording stopped");
                true
            }
            None => false,
        }
    }

    /// Join the recorder's thread, within reason.
    ///
    /// Bounded rather than a plain join: a recorder that will not finish should
    /// not be able to hold a session open for ever, and a warning about a
    /// half-written file is better than a client that never closes. It replaces
    /// a flat 500 ms sleep, which was both too long for a small file and too
    /// short for a large one.
    pub(super) fn wait_for_the_file(&self) {
        let Some(thread) = self
            .recorder_thread
            .lock()
            .expect("recorder thread lock")
            .take()
        else {
            return;
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() {
            if std::time::Instant::now() >= deadline {
                log::warn!("The recording is still being written; leaving it to finish");
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if thread.join().is_err() {
            log::warn!("Recorder thread panicked");
        }
    }
}

/// Stop whatever is in a shared recorder slot, if anything.
pub(super) fn stop_recorder(slot: &Arc<RwLock<Option<Recorder>>>) {
    if let Some(rec) = slot.read().expect("recorder lock").as_ref() {
        rec.stop();
    }
}
