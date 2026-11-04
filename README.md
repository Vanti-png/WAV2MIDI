# 🎼 WAV2MIDI

WAV / MP3 ses dosyalarını MIDI'ye dönüştüren, tamamen **Rust** ile yazılmış masaüstü uygulaması.
Analiz tamamen yerelde yapılır: Spotify'ın açık kaynak **basic-pitch** sinir ağı exe'nin içine gömülüdür, internet bağlantısı ya da harici program gerektirmez.

![WAV2MIDI ekran görüntüsü](docs/screenshot.png)

## Özellikler

- **Desteklenen formatlar:** WAV, MP3, FLAC, OGG/Vorbis, M4A/AAC
- **Üç analiz modu**
  - **Yapay zekâ (önerilen)**: Spotify basic-pitch modeli; çok sesli, enstrümandan bağımsız, şarkılarda en iyi sonuç
  - **Klasik monofonik**: vokal, ıslık, flüt, keman gibi tek sesli kayıtlar (YIN)
  - **Klasik polifonik**: temiz piyano / gitar kayıtları (harmonik toplama)
- **Anlık ayar**: hassasiyet, minimum nota süresi, eşzamanlı nota sınırı (sadeleştirme) ve atak algılama sonucu yeniden analiz gerektirmeden anında günceller
- **Piyano rulosu**: dalga formu, zaman cetveli, yakınlaştırma/kaydırma, nota üzerine gelince bilgi
- **Dinleme**: orijinal ses, sentezlenmiş MIDI veya ikisi birlikte (hizalamayı kulakla kontrol etmek için)
- **MIDI çıktısı**: tempo (BPM), kuantalama (1/4 – 1/32), 128 General MIDI enstrümanı, sabit/dinamik velocity
- Sürükle-bırak, `Ctrl+O` / `Ctrl+S` / `Boşluk` / `Enter` kısayolları
- Tek dosya, kurulum gerektirmeyen `.exe`

## Kullanım

1. [Releases](../../releases) sayfasından `wav2midi.exe` dosyasını indirip çalıştırın.
2. Bir ses dosyasını pencereye sürükleyin veya **Dosya Aç** ile seçin. Dosya otomatik analiz edilir.
3. Varsayılan **Yapay zekâ** modu çoğu kayıt için en iyisidir. Tek sesli kayıtlarda *Klasik monofonik* de denenebilir.
4. **Nota algılama** kaydırıcılarıyla sonucu ince ayarlayın, **▶ Birlikte** ile dinleyerek kontrol edin.
5. **MIDI Kaydet** ile `.mid` dosyasını kaydedin.

| Kısayol | İşlev |
|---|---|
| `Ctrl+O` | Dosya aç |
| `Ctrl+S` | MIDI kaydet |
| `Enter` | Yeniden analiz et |
| `Boşluk` | Çal / durdur |
| `Ctrl + tekerlek` | Yatay yakınlaştır |
| Tekerlek / sürükleme | Rulo üzerinde gezin |
| Tıklama | Çalma konumunu seç |

### İyi sonuç için ipuçları

- Tam şarkılarda (davul + gitar + bas) **Yapay zekâ** modunu kullanın. Distorsiyonlu gitar power chord'ları doğal olarak 2–3 nota (kök + kuint + oktav) olarak çıkar.
- Sonuç kalabalıksa **Eşzamanlı nota** kaydırıcısıyla aynı anda çalan nota sayısını 1–4 arasına indirin; en zayıf notalar atılır.
- En temiz sonuç yine de tek enstrümanlı, davulsuz kayıtlarda alınır.
- Gereksiz kısa notalar varsa **hassasiyeti düşürün** veya **min. nota süresini** artırın.
- Bas notaları ya da yavaş parçalar için polifonik modda **Pencere: Hassas** seçeneğini deneyin; hızlı pasajlar için **Hızlı**'yı kullanın.

## Nasıl çalışır?

| Aşama | Yöntem |
|---|---|
| Ses çözme | [symphonia](https://github.com/pdeljanov/Symphonia) — tümü Rust, mono'ya indirgeme |
| Yapay zekâ modu | Spotify **basic-pitch** (ICASSP 2022) ONNX modeli, saf Rust ONNX çalıştırıcısı [tract](https://github.com/sonos/tract) ile; 22.05 kHz'e pencereli-sinc yeniden örnekleme, 2 sn'lik örtüşen pencereler, basic-pitch'in nota oluşturma algoritmasının (başlangıç tepe seçimi + "melodia" izleme) Rust karşılığı |
| Monofonik perde | **YIN** algoritması (FFT tabanlı fark fonksiyonu, parabolik ara değerleme), medyan filtre, histerezisli nota bölütleme |
| Polifonik perde | Klapuri (2006) tarzı **harmonik genlik toplama**: Hann pencereli STFT → spektral beyazlatma → ağırlıklı harmonik salience → yinelemeli tahmin ve *spektral düzgünlük* ile harmonik çıkarma |
| Atak algılama | Kısa pencereli spektral akı (spectral flux) + tepe seçimi; nota başlangıçları ataklara hizalanır, tekrar çalınan notalar bölünür |
| Sadeleştirme | Eşzamanlı nota sınırı: velocity × √süre skoruna göre en güçlü notalar tutulur |
| MIDI | Bağımlılıksız SMF format 1 yazıcı (480 PPQ, tempo + ölçü izi, nota izi) |
| Arayüz | [egui / eframe](https://github.com/emilk/egui), dosya diyaloğu için [rfd](https://github.com/PolyMeilex/rfd), çalma için [rodio](https://github.com/RustAudio/rodio) |

Ağır analiz kare kare [rayon](https://github.com/rayon-rs/rayon) ile paralel yapılır; birkaç dakikalık bir dosya genellikle birkaç saniyede analiz edilir.
Analiz sonucu önbellekte tutulduğundan, nota takibi ayarları kaydırıcılar oynatıldıkça anında yeniden hesaplanır.

## Kaynaktan derleme

[Rust](https://rustup.rs) (stable) gerekir. Windows'ta MSVC araç zinciri kullanılır.

```bash
cargo build --release
```

Çıktı: `target/release/wav2midi.exe`

Testleri çalıştırmak için (sentetik melodi ve akorlarla uçtan uca doğrulama):

```bash
cargo test --release
```

### Proje yapısı

```
src/
  lib.rs            çekirdek kütüphane
  audio.rs          ses çözme (symphonia)
  resample.rs       pencereli sinc yeniden örnekleyici
  analysis/
    ai.rs           basic-pitch modeli (tract) + nota oluşturma
    mono.rs         YIN + monofonik nota takibi
    poly.rs         çoklu perde tahmini + polifonik nota takibi
    util.rs         yardımcılar (pencere, yüzdelik, tepe seçimi)
  midi.rs           SMF yazıcı
  synth.rs          önizleme sentezleyicisi
  notes.rs          Nota türü, nota adları
  main.rs / app.rs  egui arayüzü
  piano_roll.rs     piyano rulosu bileşeni
  player.rs         ses çalma
  gm.rs             General MIDI enstrüman listesi
assets/basic_pitch/ gömülü model (nmp.onnx) ve lisansı
examples/diag.rs    bir dosyayı tüm modlarla analiz edip istatistik yazdırır
tests/pipeline.rs   uçtan uca testler
```

## Sınırlamalar

- Tam şarkı mikslerinde yapay zekâ modu da tüm enstrümanları tek bir MIDI izine yazar; davullar ara sıra sahte notalar üretebilir.
- Tempo otomatik algılanmaz; MIDI dosyası seçtiğiniz BPM ile yazılır (notaların gerçek zamanlaması her durumda korunur).
- Pitch bend / perde kayması MIDI'ye aktarılmaz; notalar en yakın yarım tona yuvarlanır.

## Lisans

[MIT](LICENSE). Gömülü basic-pitch modeli Spotify AB'ye aittir ve [Apache License 2.0](assets/basic_pitch/LICENSE) ile lisanslıdır (bkz. [NOTICE](assets/basic_pitch/NOTICE.md)).
