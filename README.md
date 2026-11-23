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
- **Obrót widoku** jest stosowany dopiero po dokładnym odjęciu `kotwica − kamera`, więc nie
  zmniejsza precyzji na żadnej głębokości.
- **Transformacja względem kamery.** W każdej klatce liczone jest dokładnie `kotwica − kamera`
  i dopiero wynik trafia do `f32` jako przesunięcie + skala instancji na GPU. Kreski oglądane
  w ogromnym powiększeniu (> 2^6 względem chwili rysowania) są przeliczane w `f64` na CPU
  i przycinane do ekranu, więc krawędzie zostają ostre i nieruchome.
- **Gumka** to też kreska (kolor z alfą 0): kreski rysują się do przezroczystej warstwy, w której
  ślad gumki zeruje piksele, a warstwa jest potem nakładana na tło.
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
cargo run --release                 # projekty w ~/.local/share/infinite-canvas/projects/
cargo run --release -- inny.icnv    # otwiera jeden konkretny plik
```

| Akcja | Sterowanie |
|---|---|
| rysowanie | lewy przycisk myszy |
| przesuwanie | prawy lub środkowy przycisk + przeciągnięcie; przewijanie touchpadem |
| zoom wokół kursora | kółko myszy (Ctrl + kółko = drobniej); Ctrl + przewijanie lub szczypanie na touchpadzie |
| obrót wokół kursora | Alt + kółko myszy (7,5° na ząbek); przycisk z kątem na pasku prostuje płótno |
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
./build_android.sh --to-files   # dodatkowo kopia APK w telefonie: Pobrane/Nieskonczone-plotno.apk
```

Logi aplikacji: `~/Android/Sdk/platform-tools/adb logcat -s infinite_canvas:V`

### Pędzle

Przycisk z nazwą pędzla na pasku otwiera listę – przy każdym pędzlu jest próbka, jak rysuje
(w aktualnym kolorze, renderowana tym samym silnikiem co rysunek). Każdy pędzel pamięta własną
grubość.

| Pędzel | Działanie |
|---|---|
| Pióro | grubość z naciskiem |
| Pióro wieczne | stalówka pod kątem: grubość zależy od kierunku ruchu (i nacisku) |
| Pióro dynamiczne | szybki ruch = cieńsza linia |
| Stała grubość | zawsze ta sama grubość |
| Ołówek miękki / twardy | półprzezroczysty, z ziarnem papieru; nacisk = ciemniej |
| Marker | półprzezroczysty, ścięta końcówka, nakładane kreski ciemnieją |
| Kropkowana | kropki co ~2 grubości |
| Długopis | cienki, równy |
| Pędzel | mocno reaguje na nacisk, zwężone końce |
| Kredka / Węgiel | grube ziarno, nacisk = ciemniej |
| Zakreślacz | szeroki, przezroczysty, ścięty |
| Aerograf | miękkie, rozmyte brzegi |
| Akwarela | przezroczysta, miękka, ciemniejszy brzeg, lekka faktura |
| Spray | rozproszone kropki (te same przy każdym zoomie) |
| Wypełnienie | **stuknięcie** = wiadro z farbą: wypełnia obszar zamknięty kreskami (omija wyspy w środku); **przeciągnięcie** = rysuje wypełniony kształt |

W tym samym menu jest **Kształt**: Odręcznie / Linia / Prostokąt / Elipsa – przeciągnięcie od
rogu do rogu rysuje kształt aktualnym pędzlem (z „Wypełnieniem” – wypełniony). Linia przyciąga się
do wielokrotności 45°, prawie-kwadrat do kwadratu, prawie-koło do koła.

Wszystkie pędzle są wektorowe (ziarno ołówka liczone w shaderze we współrzędnych kreski), więc
nie tracą ostrości przy zoomie. Kreska półprzezroczysta nie ciemnieje tam, gdzie nachodzi sama
na siebie (bufor stencil).

### Zdjęcia

**Więcej → Zdjęcia…** → **Dodaj zdjęcie** pokazuje siatkę zdjęć z galerii telefonu (za pierwszym
razem Android pyta o dostęp do zdjęć; na komputerze: obrazy z `~/Obrazy`, `~/Pictures`,
`~/Pobrane` albo przeciągnięty na okno plik). Po wybraniu:

- **Do odrysowania** – zdjęcie leży pod rysunkiem (gumka go nie ściera). Od razu można je
  ustawić: jeden palec przesuwa, dwa skalują i obracają, „Gotowe” lub „Wstecz” kończy. W oknie
  Zdjęcia: suwak krycia, „Przesuń / skaluj”, „Ukryj”, „Usuń”.
- **Zamień na rysunek** – dwa style:
  - **Kolorowy**: kolory zdjęcia zredukowane do 6–20 barw (k-średnie), plamy zamienione na
    wypełnione kształty wektorowe (od największych do najmniejszych), opcjonalnie z konturami na
    wierzchu – wychodzi kolorowa ilustracja,
  - **Same kontury**: krawędzie zdjęcia (Canny) jako kreski aktualnym pędzlem, kolorem i grubością.

  Suwak „szczegóły” decyduje o liczbie barw / drobnych linii. Opcjonalnie blade zdjęcie zostaje
  pod spodem. Jedno „Cofnij” usuwa całość. W oknie Zdjęcia przy każdym zdjęciu jest miniaturka.

Zdjęcie jest zapisywane w projekcie (JPEG, maks. 2048 px).

### Warstwy

**Więcej → Warstwy…**: lista warstw (górna na górze). Stuknięcie nazwy wybiera warstwę, na której
rysujesz (także gumka, wypełnienie i rysunki ze zdjęć trafiają na nią). Każda warstwa ma krycie
(suwak), „Ukryj/Pokaż”, „W górę/W dół”, zmianę nazwy i usuwanie (z kreskami; kreski da się
przywrócić „Cofnij”). Gumka ściera tylko na swojej warstwie. Każda warstwa jest renderowana do
osobnej przezroczystej tekstury i nakładana z własnym kryciem.

### Zakładki

**Więcej → Zakładki…**: „Dodaj zakładkę” zapamiętuje obecny widok (miejsce, przybliżenie, obrót).
Stuknięcie zakładki (lub „Leć”) płynnie przenosi kamerę: oddalenie, przelot, przybliżenie – także
między miejscami odległymi o wiele rzędów wielkości, z dokładnym lądowaniem. Dotknięcie ekranu
przerywa lot. Zakładki zapisują się w projekcie.

**Wideo MP4** (w oknie Zakładki, od 2 zakładek): film z przelotem przez zakładki po kolei (postój
na każdej, płynny lot między nimi), 768 px szerokości, 30 kl./s, proporcje ekranu. Na telefonie
kodowany sprzętowo (MediaCodec H.264) i zapisywany w galerii (Filmy → „Nieskończone płótno”), na
komputerze przez `ffmpeg` do `~/Wideo/Nieskończone płótno/`. Postęp widać na dole; „Wstecz” anuluje.

### Nagrywanie

**Więcej → Nagrywaj wideo (MP4)** nagrywa płótno (rysowanie, przesuwanie, zoom – bez paska narzędzi
i okien), 30 kl./s, 768 px szerokości. Czerwony przycisk w prawym górnym rogu pokazuje czas;
stuknięcie kończy nagrywanie. Film trafia tam, gdzie wideo z zakładek (galeria → Filmy).

### Eksport

**Więcej → Eksport do PNG** zapisuje to, co widać na ekranie (bez paska narzędzi), jako PNG w podwójnej
rozdzielczości ekranu: na telefonie w galerii (Obrazy → „Nieskończone płótno”), na komputerze w
`~/Obrazy/Nieskończone płótno/`.

### Gesty

| Akcja | Gest |
|---|---|
| rysowanie | rysik albo jeden palec |
| przesuwanie + zoom | dwa palce (szczypanie wokół środka gestu), z bezwładnością po puszczeniu |
| obrót | przekręcenie dwoma palcami (zaczyna się po ~10°, żeby zwykłe szczypanie nie obracało); blisko pionu (< 6°) płótno samo się prostuje; przycisk z kątem na pasku – wyprostuj |
| gumka | narzędzie „Gumka” na pasku albo gumka na końcu rysika – ściera dokładnie ślad gumki (okrągły, wektorowy, ostry przy każdym zoomie); kreski dorysowane później nie są ścierane |
| narzędzie **Przesuń** (pasek, obok kolorów) | jeden palec / rysik przesuwa płótno i nic nie rysuje; dwa stuknięcia + przeciągnięcie w dół/górę = zoom (jak w Mapach) – wygodne do pokazywania i nagrywania |
| narzędzie **Zaznacz** (pasek, obok kolorów) | obrysuj kreski palcem (lasso); przeciągnij ramkę – przesunięcie, uchwyt w prawym dolnym rogu – zmiana rozmiaru; pasek na dole: Usuń / Duplikuj / Kolor (aktualny) / Gotowe; każda zmiana to jedno „Cofnij” |
| palm rejection | po pierwszym użyciu rysika palce już nie rysują – jeden palec przesuwa płótno |

Pociągnięcie zaczęte palcem jest anulowane, jeśli w ciągu ćwierć sekundy dołoży się drugi palec
(start szczypania).

## Projekty

Aplikacja startuje od **galerii projektów**: kafelki z miniaturką, nazwą i datą zmiany oraz kafelek
„+ Nowa kartka”. Stuknięcie w kafelek otwiera projekt; przycisk **Projekty** na pasku (albo
„Wstecz” na Androidzie) zapisuje go, odświeża miniaturkę i wraca do galerii. Przycisk „•••” pod
kafelkiem: zmiana nazwy, duplikowanie, usuwanie (z potwierdzeniem).

Każdy projekt to plik `Nazwa.icnv` + miniaturka `Nazwa.png` w folderze `projects/`
(desktop: `~/.local/share/infinite-canvas/`, Android: pamięć wewnętrzna aplikacji). Miniaturka
jest renderowana z wektorów przy wyjściu z projektu i co ~20 s podczas rysowania.

## Zapis

Rysunek zapisuje się automatycznie ~1,5 s po każdej zmianie oraz przy wyjściu/wstrzymaniu
aplikacji. Format binarny: nagłówek `ICNV`, numer wersji (u16, obecnie 8; starsze wersje nadal się
wczytują), dane w `postcard`. Zapis jest
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
