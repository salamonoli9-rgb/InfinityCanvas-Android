# Nieskończone płótno

Aplikacja do rysowania z nieskończonym płótnem i nieograniczonym przybliżaniem/oddalaniem
(w duchu Endless Paper). Kreski są wektorami renderowanymi na nowo w każdej klatce, więc
nigdy się nie rozpikselują. Ten sam kod działa na Androidzie i na desktopie Linux.

## Jak to działa (w skrócie)

- **Współrzędne o dowolnej precyzji.** Pozycje na płótnie to liczby `mantysa · 2^wykładnik`
  z mantysą dowolnej długości (`crates/core/src/coords.rs`). Dodawanie i odejmowanie są
  dokładne, więc nie ma limitu zoomu w żadną stronę.
- **Poziom skali kreski.** Kreska pamięta `floor(log2 zoomu)` z chwili rysowania, dokładną
  kotwicę (całkowitą w jednostkach swojego poziomu) i punkty jako małe `f32` względem kotwicy.
- **Transformacja względem kamery.** W każdej klatce liczone jest dokładnie `kotwica − kamera`
  i dopiero wynik trafia do `f32` jako przesunięcie + skala instancji na GPU. Kreski oglądane
  w ogromnym powiększeniu (> 2^6 względem chwili rysowania) są przeliczane w `f64` na CPU
  i przycinane do ekranu, więc krawędzie zostają ostre i nieruchome.
- **Indeks przestrzenny** per poziom skali (wielorozdzielcza siatka haszowana), pomijanie kresek
  poza ekranem, mniejszych od 0,5 px i astronomicznie większych od ekranu; cache tesselacji
  (lyon) z poziomami szczegółowości; MSAA 4×.

## Struktura

```
crates/core   logika bez GPU: współrzędne, kamera, kreski, indeks, cofnij/ponów, zapis
crates/app    renderer wgpu, gesty, pasek narzędzi egui, launchery:
              desktop.rs (winit) i android.rs (NativeActivity / android-activity)
android/      AndroidManifest.xml i zasoby (ikona, nazwa)
build_android.sh
```

## Wymagania

- Rust (stable) – `rustup`
- Desktop: sterownik Vulkan lub OpenGL (Mesa wystarczy)
- Android: Android SDK z `build-tools`, `platforms;android-36`, NDK, `platform-tools` (adb),
  `cargo install cargo-ndk`, `rustup target add aarch64-linux-android`, Java (wystarczy JRE –
  do podpisywania APK). Gradle ani JDK nie są potrzebne.

Skrypt domyślnie szuka SDK w `~/Android/Sdk` (można nadpisać zmienną `ANDROID_HOME`).

## Desktop

```bash
cargo run --release                 # rysunek w ~/.local/share/infinite-canvas/canvas.icnv
cargo run --release -- inny.icnv    # inny plik
```

| Akcja | Sterowanie |
|---|---|
| rysowanie | lewy przycisk myszy |
| przesuwanie | prawy lub środkowy przycisk + przeciągnięcie; przewijanie touchpadem |
| zoom wokół kursora | kółko myszy (Ctrl + kółko = drobniej); Ctrl + przewijanie lub szczypanie na touchpadzie |
| cofnij / ponów | Ctrl+Z / Ctrl+Shift+Z lub Ctrl+Y |
| pióro / gumka | P / E |
| powrót do początku | Home |

## Android

1. Na telefonie włącz **Opcje programisty** (Ustawienia → Informacje o telefonie → 7× stuknij
   „Numer kompilacji”), potem w Opcjach programisty włącz **Debugowanie USB**.
2. Podłącz telefon kablem i zaakceptuj na nim okno „Zezwolić na debugowanie USB?”.
3. Sprawdź: `~/Android/Sdk/platform-tools/adb devices` – telefon powinien mieć status `device`.
4. Zbuduj i zainstaluj:

```bash
./build_android.sh              # release, instaluje i uruchamia
./build_android.sh --debug      # szybsza kompilacja
./build_android.sh --no-install # tylko target/android/infinite-canvas.apk
```

Logi aplikacji: `~/Android/Sdk/platform-tools/adb logcat -s infinite_canvas:V`

### Gesty

| Akcja | Gest |
|---|---|
| rysowanie | rysik albo jeden palec |
| przesuwanie + zoom | dwa palce (szczypanie wokół środka gestu), z bezwładnością po puszczeniu |
| gumka | narzędzie „Gumka” na pasku albo gumka na końcu rysika |
| palm rejection | po pierwszym użyciu rysika palce już nie rysują – jeden palec przesuwa płótno |

Pociągnięcie zaczęte palcem jest anulowane, jeśli w ciągu ćwierć sekundy dołoży się drugi palec
(start szczypania).

## Zapis

Rysunek zapisuje się automatycznie ~1,5 s po każdej zmianie oraz przy wyjściu/wstrzymaniu
aplikacji. Format binarny: nagłówek `ICNV`, numer wersji (u16), dane w `postcard`. Zapis jest
atomowy (plik tymczasowy + rename). Uszkodzony plik nie jest nadpisywany – zostaje przemianowany
na `*.broken`.

## Testy

```bash
cargo test                      # w tym testy renderowania offscreen na GPU
cargo clippy --all-targets
cargo test --release -p infinite_canvas --test render_offscreen -- --ignored --nocapture   # wydajność 10k kresek
```

Testy obejmują m.in. zejście kamerą na 2^±300 i powrót, rysowanie na głębokości ~10^90 i
sprawdzenie pikseli, przesunięcia o 0,5 px bez drgań, dokładność krawędzi przy powiększeniu 2^20,
zapis/odczyt ogromnych współrzędnych, gumkę i cofanie.

## Licencja

MIT OR Apache-2.0. Projekty w `ref/` (afnopadd, InfiniPaint – oba GPL-3.0) służyły wyłącznie
jako inspiracja; ich kod nie jest używany ani kopiowany.
