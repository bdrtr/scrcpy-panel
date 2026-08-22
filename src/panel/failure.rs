//! Turning a failure into a card the user can act on.
//!
//! The panel used to show one error card — "Cihaz listesi alınamadı" over the
//! raw output — for every failure alike. The mockup has eight, each naming what
//! went wrong and offering the two things worth trying. This is the part that
//! decides which one to show: it reads the text adb or the session produced and
//! matches it against the failures this program can actually cause.
//!
//! The raw output is still shown underneath. Nothing here replaces it, because
//! a guess that lands on the wrong card must not hide what really happened.

/// What the panel offers to do about a failure, beyond rescanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Nothing specific to offer; the card shows only "Yeniden tara".
    None,
    RestartAdb,
    PickAdbPath,
    ListEncoders,
    OpenSettings,
}

impl Remedy {
    /// The label of the secondary button, or none when there is nothing to add.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Remedy::None => None,
            Remedy::RestartAdb => Some("adb sunucusunu yeniden başlat"),
            Remedy::PickAdbPath => Some("adb yolunu seç"),
            Remedy::ListEncoders => Some("Kodlayıcıları listele"),
            Remedy::OpenSettings => Some("Ayarları aç"),
        }
    }
}

/// One error card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The kicker above the title, as in "Hata · yetki".
    pub tag: &'static str,
    pub title: &'static str,
    /// What to do about it, in a sentence.
    pub detail: &'static str,
    pub remedy: Remedy,
}

/// Match a failure against the ones this program can cause.
///
/// Order matters: the specific phrases come first, so that a message naming
/// both a device and a port lands on the port card rather than the generic one.
pub fn classify(text: &str) -> Failure {
    let lower = text.to_lowercase();
    let has = |needle: &str| lower.contains(needle);

    // Two of the user's own boxes, refused before any device work. It goes
    // first because the message names audio twice and used to land on the audio
    // card below, which says the Android version may be too old, advises
    // turning audio off — it already is — and offers nothing to press.
    if has("contradict") {
        return Failure {
            tag: "Hata · ayar",
            title: "Seçenekler çelişiyor",
            detail: "Sesi kapatan seçenek ile ses olmadan çıkmayı isteyen seçenek bir arada \
                     işaretli. Ses bölümünde ikisinden birini kaldırın.",
            remedy: Remedy::OpenSettings,
        };
    }

    // Several devices and no serial. Above the no-device card, not below it:
    // `Session::start` wraps every outcome of device selection in "Device
    // selection failed", this one included, and that phrase used to be enough
    // for the card below to claim it — so two phones plugged in were reported
    // as none, with advice to plug one in.
    if has("more than one device")
        || has("multiple devices")
        || has("devices connected")
        || has("use --serial")
    {
        return Failure {
            tag: "Hata · cihaz",
            title: "Birden fazla cihaz bağlı",
            detail: "Hangisinin yansıtılacağı belirsiz. Cihazlar sekmesinden birini işaretleyin.",
            remedy: Remedy::None,
        };
    }

    // The client says the first when nothing is plugged in, and the second when
    // something is but the filter — --select-usb, --select-tcpip — excluded all
    // of it.
    if has("no device") || has("device among") {
        return Failure {
            tag: "Hata · cihaz",
            title: "Cihaz bulunamadı",
            detail: "USB kablosunu takın ve cihazdaki USB hata ayıklama istemini onaylayın. \
                     Kablosuz bir cihazı Cihazlar sekmesindeki TCP/IP alanından bağlayın.",
            remedy: Remedy::RestartAdb,
        };
    }

    if has("unauthorized") {
        return Failure {
            tag: "Hata · yetki",
            title: "Cihaz yetkilendirilmemiş",
            detail: "Cihaz ekranındaki \"Bu bilgisayara izin ver\" istemini onaylayın. \
                     İstem çıkmıyorsa USB hata ayıklamayı kapatıp açın.",
            remedy: Remedy::RestartAdb,
        };
    }

    // The third is adb's own words for a serial that was there a moment ago:
    // "device 'R58M31XABCD' not found". It used to reach the adb card at the
    // bottom, which offers to go looking for an adb that is working.
    if has("offline")
        || has("connection closed")
        || has("connection reset")
        || (has("device '") && has("not found"))
    {
        return Failure {
            tag: "Hata · bağlantı",
            title: "Cihaz çevrimdışı",
            detail: "Bağlantı koptu. Kabloyu ve USB kipini denetleyin; kablosuz bir \
                     cihazda yeniden bağlanmak gerekebilir.",
            remedy: Remedy::None,
        };
    }

    // The server checks this itself and refuses to start, so the wording comes
    // from it: "The server version (x) does not match the client".
    if has("server version") || has("does not match") {
        return Failure {
            tag: "Hata · sürüm",
            title: "scrcpy sürümü uyumsuz",
            detail: "Cihazdaki sunucu ile istemci aynı sürüm olmalı. \
                     Ayarlar'dan bu istemcinin beklediği sürümde bir scrcpy-server seçin.",
            remedy: Remedy::OpenSettings,
        };
    }

    // Not adb, and not a version mismatch: there is no scrcpy-server to send.
    // The bare words "not found" used to carry this to the adb card, so a
    // missing server file was reported as a missing adb and answered with a
    // file picker for the adb that had just worked.
    if has("scrcpy-server not found") {
        return Failure {
            tag: "Hata · sunucu",
            title: "scrcpy-server bulunamadı",
            detail: "Cihaza gönderilecek sunucu dosyası yok. scrcpy paketini kurun ya da \
                     scrcpy-server dosyasını çalıştırılabilir dosyanın yanına koyun; \
                     indirme adresi aşağıdaki çıktıda.",
            remedy: Remedy::None,
        };
    }

    if has("encoder") {
        return Failure {
            tag: "Hata · kodlayıcı",
            title: "Kodlayıcı bulunamadı",
            detail: "Seçilen kodlayıcı bu cihazda yok. Cihazın listesinden birini seçin \
                     ya da kodlayıcı alanını boş bırakın.",
            remedy: Remedy::ListEncoders,
        };
    }

    if has("audio") {
        return Failure {
            tag: "Uyarı · ses",
            title: "Ses aktarılamıyor",
            detail: "Ses aktarımı Android 11 ve üstünü gerektirir, ve bazı cihazlar \
                     yakalamayı engeller. Sesi kapatıp yansıtmayı sürdürebilirsiniz.",
            remedy: Remedy::None,
        };
    }

    // Both the recorder and the screenshot write files, and both fail this way.
    if has("permission denied") || has("failed to open output") || has("read-only") {
        return Failure {
            tag: "Hata · kayıt",
            title: "Dosya yazılamıyor",
            detail: "Hedef klasör yazılabilir olmalı. Ayarlar'daki kayıt klasörünü \
                     değiştirin ya da izinleri düzeltin.",
            remedy: Remedy::OpenSettings,
        };
    }

    // "Could not set up reverse tunnel on ports 27183..27199" is what this
    // client bails with when the range is exhausted; the other three phrases
    // are upstream C scrcpy's and adb's. Without the first, the one failure
    // this card exists for fell through to the generic card.
    if has("tunnel on ports")
        || has("already in use")
        || has("could not find any port")
        || has("bind")
    {
        return Failure {
            tag: "Hata · port",
            title: "Port kullanımda",
            detail: "Bu aralıktaki portlar başka bir program tarafından tutuluyor. \
                     Başka bir scrcpy açıksa kapatın ya da port aralığını değiştirin.",
            remedy: Remedy::RestartAdb,
        };
    }

    // The daemon is a different failure from the binary: one is not running,
    // the other is not there at all.
    if has("adb daemon") || has("start-server") {
        return Failure {
            tag: "Hata · adb",
            title: "adb sunucusu çalışmıyor",
            detail: "Cihazlara adb sunucusu üzerinden ulaşılıyor ve şu an cevap vermiyor.",
            remedy: Remedy::RestartAdb,
        };
    }

    // "not found" on its own used to be here as well, and it was too much: it
    // claimed the missing server file, a device that had gone away, and
    // "Video codec not found in FFmpeg" — none of which is adb, and all of
    // which were answered with a file picker for it. A missing binary shows up
    // as the io error below.
    if has("no such file") || has("program not found") || has("os error 2") {
        return Failure {
            tag: "Hata · adb",
            title: "adb bulunamadı",
            detail: "adb çalıştırılamadı. Ayarlar'dan tam yolunu verin ya da \
                     platform-tools paketini kurun.",
            remedy: Remedy::PickAdbPath,
        };
    }

    Failure {
        tag: "Hata",
        title: "İşlem başarısız",
        detail: "Ayrıntı aşağıdaki çıktıda. Günlük sekmesinde daha fazlası olabilir.",
        remedy: Remedy::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of these is a message this program or adb really produces.
    ///
    /// "Really" is the word that had to be earned: the port case used to be
    /// upstream C scrcpy's wording, which nothing here can emit, so the card
    /// was green in the test and unreachable in the program.
    const REAL_MESSAGES: &[(&str, &str)] = &[
            (
                "error: device unauthorized.\nThis adb server's $ADB_VENDOR_KEYS is not set",
                "Cihaz yetkilendirilmemiş",
            ),
            ("R58M31XABCD offline usb:1-3", "Cihaz çevrimdışı"),
            (
                "Device selection failed: No device connected. Plug in your phone and \
                 enable USB debugging.",
                "Cihaz bulunamadı",
            ),
            (
                "adb: error: failed to get feature set: more than one device/emulator",
                "Birden fazla cihaz bağlı",
            ),
            (
                "Device selection failed: 2 devices connected. Use --serial to select one: \
                 [\"R58M31XABCD\", \"192.168.1.42:5555\"]",
                "Birden fazla cihaz bağlı",
            ),
            (
                "Device selection failed: No USB device among [\"192.168.1.42:5555\"]",
                "Cihaz bulunamadı",
            ),
            (
                "--require-audio and --no-audio contradict each other",
                "Seçenekler çelişiyor",
            ),
            (
                "scrcpy-server not found. Download from:\n\
                 https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-server-v4.1\n\
                 and place it next to the executable.",
                "scrcpy-server bulunamadı",
            ),
            (
                "Failed to push /usr/share/scrcpy/scrcpy-server to /data/local/tmp: \
                 ADB error: device 'R58M31XABCD' not found",
                "Cihaz çevrimdışı",
            ),
            (
                "ERROR: The server version (2.4) does not match the client (4.1)",
                "scrcpy sürümü uyumsuz",
            ),
            (
                "ERROR: Encoder 'c2.android.hevc.encoder' not found",
                "Kodlayıcı bulunamadı",
            ),
            (
                "ERROR: Could not capture audio, audio disabled",
                "Ses aktarılamıyor",
            ),
            (
                "Failed to open output file: Permission denied",
                "Dosya yazılamıyor",
            ),
            (
                "Failed to open ADB tunnel: Could not set up forward tunnel on ports \
                 27183..27199",
                "Port kullanımda",
            ),
            (
                "Failed to connect to ADB daemon on 127.0.0.1:5037.",
                "adb sunucusu çalışmıyor",
            ),
            (
                "Failed to run adb: No such file or directory (os error 2)",
                "adb bulunamadı",
            ),
    ];

    #[test]
    fn real_messages_land_on_the_card_that_names_them() {
        for (text, title) in REAL_MESSAGES {
            assert_eq!(classify(text).title, *title, "for {text:?}");
        }
    }

    /// Every word on a card exists in the .po.
    ///
    /// `show_failure` translates the four strings where they meet the
    /// interface — `tr!(card.title)` and the rest — and an argument that is not
    /// a literal is invisible to the scanner in `i18n.rs`, so the cards are
    /// checked here instead. Without this a new card is Turkish in an English
    /// panel and nothing says so.
    #[test]
    fn every_card_can_be_translated() {
        let po = include_str!("../../lang/en/LC_MESSAGES/scrcpy-slint.po");
        let mut missing = Vec::new();
        let mut check = |text: &str| {
            let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
            if !po.contains(&format!("msgid \"{escaped}\"")) {
                missing.push(text.to_string());
            }
        };
        for (text, _) in REAL_MESSAGES {
            let card = classify(text);
            check(card.tag);
            check(card.title);
            check(card.detail);
            if let Some(label) = card.remedy.label() {
                check(label);
            }
        }
        let generic = classify("something nobody has seen before");
        check(generic.tag);
        check(generic.title);
        check(generic.detail);
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "{} card string(s) with no msgid:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    /// An unauthorized device also mentions "adb"; the specific card has to win.
    #[test]
    fn the_specific_card_wins_over_the_generic_one() {
        let text = "adb: error: device unauthorized";
        assert_eq!(classify(text).title, "Cihaz yetkilendirilmemiş");
    }

    #[test]
    fn an_unrecognised_failure_still_gets_a_card() {
        let failure = classify("something nobody has seen before");
        assert_eq!(failure.title, "İşlem başarısız");
        assert_eq!(failure.remedy, Remedy::None);
        assert_eq!(failure.remedy.label(), None, "no button with nothing to do");
    }

}
