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

    /// The key handed to the UI and back, so the click knows what to run.
    pub fn key(self) -> &'static str {
        match self {
            Remedy::None => "",
            Remedy::RestartAdb => "restart-adb",
            Remedy::PickAdbPath => "pick-adb",
            Remedy::ListEncoders => "list-encoders",
            Remedy::OpenSettings => "settings",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "restart-adb" => Remedy::RestartAdb,
            "pick-adb" => Remedy::PickAdbPath,
            "list-encoders" => Remedy::ListEncoders,
            "settings" => Remedy::OpenSettings,
            _ => Remedy::None,
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

    // The client says this itself when nothing is plugged in, and adb says the
    // second one when several are and no serial was given.
    if has("no device") || has("device selection failed") {
        return Failure {
            tag: "Hata · cihaz",
            title: "Cihaz bulunamadı",
            detail: "USB kablosunu takın ve cihazdaki USB hata ayıklama istemini onaylayın. \
                     Kablosuz bir cihazı Cihazlar sekmesindeki TCP/IP alanından bağlayın.",
            remedy: Remedy::RestartAdb,
        };
    }

    if has("more than one device") || has("multiple devices") {
        return Failure {
            tag: "Hata · cihaz",
            title: "Birden fazla cihaz bağlı",
            detail: "Hangisinin yansıtılacağı belirsiz. Cihazlar sekmesinden birini işaretleyin.",
            remedy: Remedy::None,
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

    if has("offline") || has("connection closed") || has("connection reset") {
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

    if has("already in use") || has("could not find any port") || has("bind") {
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

    if has("no such file") || has("not found") || has("program not found") || has("os error 2")
    {
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
    #[test]
    fn real_messages_land_on_the_card_that_names_them() {
        let cases: &[(&str, &str)] = &[
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
                "Could not find any port in range 27183:27199",
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
        for (text, title) in cases {
            assert_eq!(classify(text).title, *title, "for {text:?}");
        }
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

    /// The key travels to the UI as a string and comes back on the click.
    #[test]
    fn every_remedy_survives_the_round_trip_through_its_key() {
        for remedy in [
            Remedy::None,
            Remedy::RestartAdb,
            Remedy::PickAdbPath,
            Remedy::ListEncoders,
            Remedy::OpenSettings,
        ] {
            assert_eq!(Remedy::from_key(remedy.key()), remedy);
        }
    }
}
