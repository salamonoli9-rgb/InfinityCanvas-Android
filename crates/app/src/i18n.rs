//! Interface languages. Polish texts are the keys (the code stays readable); every other
//! language is a column of [`TABLE`]. Anything missing falls back to Polish.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Pl,
    En,
    De,
    Ru,
    Uk,
    Es,
    Fr,
    It,
}

impl Lang {
    pub const ALL: [Lang; 8] = [Lang::Pl, Lang::En, Lang::De, Lang::Ru, Lang::Uk, Lang::Es, Lang::Fr, Lang::It];

    /// The language's own name (shown in the language list).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Pl => "Polski",
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Ru => "Русский",
            Lang::Uk => "Українська",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::It => "Italiano",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Pl => "pl",
            Lang::En => "en",
            Lang::De => "de",
            Lang::Ru => "ru",
            Lang::Uk => "uk",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::It => "it",
        }
    }

    /// From an ISO 639-1 code or a locale like "de_DE.UTF-8"; unknown → English.
    pub fn from_code(code: &str) -> Lang {
        let c = code.get(..2).unwrap_or("").to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == c).unwrap_or(Lang::En)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

fn index() -> &'static HashMap<&'static str, usize> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    INDEX.get_or_init(|| TABLE.iter().enumerate().map(|(i, (k, _))| (*k, i)).collect())
}

/// The text `pl` in `lang`.
pub fn t_in(lang: Lang, pl: &'static str) -> &'static str {
    if lang == Lang::Pl {
        return pl;
    }
    match index().get(pl) {
        Some(&i) => TABLE[i].1[lang as usize - 1],
        None => pl,
    }
}

/// The text `pl` in the current language.
pub fn t(pl: &'static str) -> &'static str {
    t_in(current(), pl)
}

/// [`t`] for texts that aren't literals (e.g. messages passed along as `String`).
pub fn t_dyn(pl: &str) -> String {
    let lang = current();
    match index().get_key_value(pl) {
        Some((k, _)) => t_in(lang, k).to_string(),
        None => pl.to_string(),
    }
}

/// [`t`] with `{}` placeholders filled in order.
pub fn tf(pl: &'static str, args: &[&dyn Display]) -> String {
    tf_in(current(), pl, args)
}

pub fn tf_in(lang: Lang, pl: &'static str, args: &[&dyn Display]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut parts = t_in(lang, pl).split("{}").peekable();
    while let Some(part) = parts.next() {
        out.push_str(part);
        if parts.peek().is_some()
            && let Some(a) = args.next()
        {
            out.push_str(&a.to_string());
        }
    }
    out
}

/// Polish → [English, German, Russian, Ukrainian, Spanish, French, Italian].
static TABLE: &[(&str, [&str; 7])] = &[
    // Toolbar and tools
    ("Projekty", ["Projects", "Projekte", "Проекты", "Проєкти", "Proyectos", "Projets", "Progetti"]),
    ("Wróć do listy projektów", ["Back to the projects", "Zurück zu den Projekten", "К списку проектов", "До списку проєктів", "Volver a los proyectos", "Retour aux projets", "Torna ai progetti"]),
    ("Gumka", ["Eraser", "Radierer", "Ластик", "Гумка", "Goma", "Gomme", "Gomma"]),
    ("Cofnij", ["Undo", "Rückgängig", "Отменить", "Скасувати", "Deshacer", "Annuler", "Annulla"]),
    ("Ponów", ["Redo", "Wiederholen", "Повторить", "Повторити", "Rehacer", "Rétablir", "Ripeti"]),
    ("Początek", ["Home", "Start", "Начало", "Початок", "Inicio", "Début", "Inizio"]),
    ("Wróć do początku (Home)", ["Back to the start (Home)", "Zurück zum Anfang (Pos1)", "Вернуться к началу (Home)", "Повернутися на початок (Home)", "Volver al inicio (Inicio)", "Revenir au début (Début)", "Torna all'inizio (Home)"]),
    ("Więcej", ["More", "Mehr", "Ещё", "Ще", "Más", "Plus", "Altro"]),
    ("Zdjęcia…", ["Photos…", "Fotos…", "Фото…", "Фото…", "Fotos…", "Photos…", "Foto…"]),
    ("Warstwy…", ["Layers…", "Ebenen…", "Слои…", "Шари…", "Capas…", "Calques…", "Livelli…"]),
    ("Zakładki…", ["Bookmarks…", "Lesezeichen…", "Закладки…", "Закладки…", "Marcadores…", "Signets…", "Segnalibri…"]),
    ("Ustawienia…", ["Settings…", "Einstellungen…", "Настройки…", "Налаштування…", "Ajustes…", "Réglages…", "Impostazioni…"]),
    ("Eksport do PNG", ["Export to PNG", "Als PNG exportieren", "Экспорт в PNG", "Експорт у PNG", "Exportar a PNG", "Exporter en PNG", "Esporta in PNG"]),
    ("Nagrywaj wideo (MP4)", ["Record video (MP4)", "Video aufnehmen (MP4)", "Записать видео (MP4)", "Записати відео (MP4)", "Grabar vídeo (MP4)", "Enregistrer une vidéo (MP4)", "Registra video (MP4)"]),
    ("Nagrywa płótno – rysowanie, przesuwanie, zoom – bez paska narzędzi", ["Records the canvas – drawing, panning, zooming – without the toolbar", "Nimmt die Leinwand auf – Zeichnen, Verschieben, Zoomen – ohne Werkzeugleiste", "Записывает холст – рисование, сдвиг, масштаб – без панели инструментов", "Записує полотно – малювання, зсув, масштаб – без панелі інструментів", "Graba el lienzo – dibujo, desplazamiento, zoom – sin la barra de herramientas", "Enregistre la toile – dessin, déplacement, zoom – sans la barre d'outils", "Registra la tela – disegno, spostamento, zoom – senza la barra degli strumenti"]),
    ("Pokaż pasek narzędzi", ["Show the toolbar", "Werkzeugleiste zeigen", "Показать панель инструментов", "Показати панель інструментів", "Mostrar la barra de herramientas", "Afficher la barre d'outils", "Mostra la barra degli strumenti"]),
    ("Schowaj pasek narzędzi", ["Hide the toolbar", "Werkzeugleiste ausblenden", "Скрыть панель инструментов", "Сховати панель інструментів", "Ocultar la barra de herramientas", "Masquer la barre d'outils", "Nascondi la barra degli strumenti"]),
    ("Chowaj menu podczas rysowania", ["Hide the menu while drawing", "Menü beim Zeichnen ausblenden", "Скрывать меню во время рисования", "Ховати меню під час малювання", "Ocultar el menú al dibujar", "Masquer le menu pendant le dessin", "Nascondi il menu mentre disegni"]),
    ("Szybkie przyciski…", ["Quick buttons…", "Schnelltasten…", "Быстрые кнопки…", "Швидкі кнопки…", "Botones rápidos…", "Boutons rapides…", "Pulsanti rapidi…"]),
    ("Szybkie przyciski", ["Quick buttons", "Schnelltasten", "Быстрые кнопки", "Швидкі кнопки", "Botones rápidos", "Boutons rapides", "Pulsanti rapidi"]),
    ("Wybierz, co pokazuje każdy kwadracik obok menu.", ["Choose what each square next to the menu does.", "Wähle, was jedes Kästchen neben dem Menü macht.", "Выберите, что делает каждый квадратик рядом с меню.", "Виберіть, що робить кожен квадратик біля меню.", "Elige qué hace cada cuadrito junto al menú.", "Choisis ce que fait chaque carré à côté du menu.", "Scegli cosa fa ogni quadratino accanto al menu."]),
    ("Przycisk {}", ["Button {}", "Taste {}", "Кнопка {}", "Кнопка {}", "Botón {}", "Bouton {}", "Pulsante {}"]),
    ("Domyślne", ["Defaults", "Standard", "По умолчанию", "За замовчуванням", "Predeterminados", "Par défaut", "Predefiniti"]),
    ("Rysuj", ["Draw", "Zeichnen", "Рисовать", "Малювати", "Dibujar", "Dessiner", "Disegna"]),
    ("Siatka", ["Grid", "Raster", "Сетка", "Сітка", "Cuadrícula", "Grille", "Griglia"]),
    ("Puste", ["Empty", "Leer", "Пусто", "Порожньо", "Vacío", "Vide", "Vuoto"]),
    ("Siatka (kształty się przyciągają)", ["Grid (shapes snap to it)", "Raster (Formen rasten ein)", "Сетка (фигуры примагничиваются)", "Сітка (фігури прилипають)", "Cuadrícula (las formas se ajustan)", "Grille (les formes s'y aimantent)", "Griglia (le forme si agganciano)"]),
    ("Zaznacz", ["Select", "Auswählen", "Выделить", "Виділити", "Seleccionar", "Sélection", "Seleziona"]),
    ("Obrysuj kreski palcem, potem przeciągnij je albo zmień rozmiar uchwytem w rogu", ["Circle strokes with your finger, then drag them or resize with the corner handle", "Striche mit dem Finger umkreisen, dann ziehen oder mit dem Eckgriff skalieren", "Обведите штрихи пальцем, затем перетащите их или измените размер уголком", "Обведіть штрихи пальцем, потім перетягніть їх або змініть розмір кутиком", "Rodea trazos con el dedo, luego arrástralos o cambia su tamaño con la esquina", "Entourez des traits du doigt, puis déplacez-les ou redimensionnez-les par le coin", "Cerchia i tratti con il dito, poi trascinali o ridimensionali con l'angolo"]),
    ("Tekst", ["Text", "Text", "Текст", "Текст", "Texto", "Texte", "Testo"]),
    ("Stuknij w płótno, żeby wpisać tekst (wektorowy)", ["Tap the canvas to type text (vector)", "Auf die Leinwand tippen, um Text einzugeben (Vektor)", "Коснитесь холста, чтобы ввести текст (векторный)", "Торкніться полотна, щоб ввести текст (векторний)", "Toca el lienzo para escribir texto (vectorial)", "Touchez la toile pour écrire du texte (vectoriel)", "Tocca la tela per scrivere un testo (vettoriale)"]),
    ("Wygnij", ["Bend", "Biegen", "Изгиб", "Вигин", "Doblar", "Courber", "Piega"]),
    ("Przeciągnij po kreskach, żeby je wygiąć (zasięg – suwak)", ["Drag over strokes to bend them (reach – slider)", "Über Striche ziehen, um sie zu biegen (Reichweite – Regler)", "Проведите по штрихам, чтобы изогнуть их (радиус – ползунок)", "Проведіть по штрихах, щоб вигнути їх (радіус – повзунок)", "Arrastra sobre los trazos para doblarlos (alcance – control)", "Glissez sur les traits pour les courber (portée – curseur)", "Trascina sui tratti per piegarli (raggio – cursore)"]),
    ("Miarka", ["Ruler", "Lineal", "Линейка", "Лінійка", "Regla", "Règle", "Righello"]),
    ("Przeciągnij, żeby zmierzyć odległość i kąt", ["Drag to measure a distance and an angle", "Ziehen, um Abstand und Winkel zu messen", "Проведите, чтобы измерить расстояние и угол", "Проведіть, щоб виміряти відстань і кут", "Arrastra para medir una distancia y un ángulo", "Glissez pour mesurer une distance et un angle", "Trascina per misurare distanza e angolo"]),
    ("Przesuń", ["Move", "Bewegen", "Двигать", "Рухати", "Mover", "Déplacer", "Sposta"]),
    ("Jeden palec przesuwa płótno, nic nie rysuje. Dwa stuknięcia i przeciągnięcie w górę/dół – zoom.", ["One finger moves the canvas, nothing is drawn. Double-tap and drag up/down to zoom.", "Ein Finger verschiebt die Leinwand, nichts wird gezeichnet. Doppeltippen und hoch/runter ziehen zum Zoomen.", "Один палец двигает холст, ничего не рисуется. Двойное касание и движение вверх/вниз – масштаб.", "Один палець рухає полотно, нічого не малюється. Подвійний дотик і рух угору/вниз – масштаб.", "Un dedo mueve el lienzo, no se dibuja nada. Doble toque y arrastrar arriba/abajo para hacer zoom.", "Un doigt déplace la toile, rien n'est dessiné. Double appui puis glisser haut/bas pour zoomer.", "Un dito sposta la tela, non si disegna. Doppio tocco e trascina su/giù per lo zoom."]),
    ("grubość", ["width", "Stärke", "толщина", "товщина", "grosor", "épaisseur", "spessore"]),
    ("rozmiar", ["size", "Größe", "размер", "розмір", "tamaño", "taille", "dimensione"]),
    ("rozmiar tekstu", ["text size", "Textgröße", "размер текста", "розмір тексту", "tamaño del texto", "taille du texte", "dimensione del testo"]),
    ("zasięg", ["reach", "Reichweite", "радиус", "радіус", "alcance", "portée", "raggio"]),
    ("Kształt (aktualnym pędzlem)", ["Shape (with the current brush)", "Form (mit dem aktuellen Pinsel)", "Фигура (текущей кистью)", "Фігура (поточним пензлем)", "Forma (con el pincel actual)", "Forme (avec le pinceau actuel)", "Forma (con il pennello attuale)"]),
    ("Odręcznie", ["Freehand", "Freihand", "От руки", "Від руки", "A mano alzada", "Main levée", "A mano libera"]),
    ("Linia", ["Line", "Linie", "Линия", "Лінія", "Línea", "Ligne", "Linea"]),
    ("Prostokąt", ["Rectangle", "Rechteck", "Прямоугольник", "Прямокутник", "Rectángulo", "Rectangle", "Rettangolo"]),
    ("Elipsa", ["Ellipse", "Ellipse", "Эллипс", "Еліпс", "Elipse", "Ellipse", "Ellisse"]),
    ("Obrót płótna – stuknij, aby wyprostować", ["Canvas rotation – tap to straighten", "Drehung – zum Begradigen tippen", "Поворот холста – коснитесь, чтобы выпрямить", "Поворот полотна – торкніться, щоб вирівняти", "Rotación del lienzo – toca para enderezar", "Rotation de la toile – touchez pour redresser", "Rotazione della tela – tocca per raddrizzare"]),
    ("Pędzel", ["Brush", "Pinsel", "Кисть", "Пензель", "Pincel", "Pinceau", "Pennello"]),
    // Brushes
    ("Pióro", ["Pen", "Stift", "Перо", "Перо", "Pluma", "Stylo", "Penna"]),
    ("Pióro wieczne", ["Fountain pen", "Füller", "Перьевая ручка", "Перова ручка", "Pluma estilográfica", "Stylo plume", "Stilografica"]),
    ("Pióro dynamiczne", ["Dynamic pen", "Dynamischer Stift", "Динамическое перо", "Динамічне перо", "Pluma dinámica", "Stylo dynamique", "Penna dinamica"]),
    ("Stała grubość", ["Fixed width", "Feste Stärke", "Постоянная толщина", "Стала товщина", "Grosor fijo", "Épaisseur fixe", "Spessore fisso"]),
    ("Ołówek miękki", ["Soft pencil", "Weicher Bleistift", "Мягкий карандаш", "М'який олівець", "Lápiz blando", "Crayon gras", "Matita morbida"]),
    ("Ołówek twardy", ["Hard pencil", "Harter Bleistift", "Твёрдый карандаш", "Твердий олівець", "Lápiz duro", "Crayon dur", "Matita dura"]),
    ("Marker", ["Marker", "Marker", "Маркер", "Маркер", "Rotulador", "Marqueur", "Pennarello"]),
    ("Kropkowana", ["Dotted", "Gepunktet", "Пунктир", "Пунктир", "Punteada", "Pointillés", "Punteggiata"]),
    ("Wypełnienie", ["Fill", "Füllung", "Заливка", "Заливка", "Relleno", "Remplissage", "Riempimento"]),
    ("Długopis", ["Ballpoint", "Kugelschreiber", "Шариковая ручка", "Кулькова ручка", "Bolígrafo", "Stylo bille", "Biro"]),
    ("Zakreślacz", ["Highlighter", "Textmarker", "Текстовыделитель", "Текстовиділювач", "Subrayador", "Surligneur", "Evidenziatore"]),
    ("Kredka", ["Crayon", "Wachsmalstift", "Мелок", "Крейда", "Cera", "Craie grasse", "Pastello"]),
    ("Węgiel", ["Charcoal", "Kohle", "Уголь", "Вугілля", "Carboncillo", "Fusain", "Carboncino"]),
    ("Aerograf", ["Airbrush", "Airbrush", "Аэрограф", "Аерограф", "Aerógrafo", "Aérographe", "Aerografo"]),
    ("Akwarela", ["Watercolour", "Aquarell", "Акварель", "Акварель", "Acuarela", "Aquarelle", "Acquerello"]),
    ("Spray", ["Spray", "Spray", "Спрей", "Спрей", "Spray", "Spray", "Spray"]),
    ("Wieczne", ["Fountain", "Füller", "Перьевая", "Перова", "Estilográfica", "Plume", "Stilo"]),
    ("Dynamiczne", ["Dynamic", "Dynamisch", "Динамич.", "Динаміч.", "Dinámica", "Dynamique", "Dinamica"]),
    ("Stałe", ["Fixed", "Fest", "Ровная", "Рівна", "Fija", "Fixe", "Fissa"]),
    ("Ołówek M", ["Pencil S", "Bleistift W", "Карандаш М", "Олівець М", "Lápiz B", "Crayon G", "Matita M"]),
    ("Ołówek T", ["Pencil H", "Bleistift H", "Карандаш Т", "Олівець Т", "Lápiz D", "Crayon D", "Matita D"]),
    ("Kropki", ["Dots", "Punkte", "Точки", "Крапки", "Puntos", "Points", "Punti"]),
    ("Wypełnij", ["Fill", "Füllen", "Заливка", "Заливка", "Rellenar", "Remplir", "Riempi"]),
    // Gallery and projects
    ("Nowa kartka", ["New page", "Neue Seite", "Новый лист", "Новий аркуш", "Hoja nueva", "Nouvelle page", "Nuovo foglio"]),
    ("Opcje", ["Options", "Optionen", "Параметры", "Параметри", "Opciones", "Options", "Opzioni"]),
    ("Opcje projektu", ["Project options", "Projektoptionen", "Параметры проекта", "Параметри проєкту", "Opciones del proyecto", "Options du projet", "Opzioni del progetto"]),
    ("Nazwa", ["Name", "Name", "Имя", "Назва", "Nombre", "Nom", "Nome"]),
    ("Zmień nazwę", ["Rename", "Umbenennen", "Переименовать", "Перейменувати", "Renombrar", "Renommer", "Rinomina"]),
    ("Duplikuj", ["Duplicate", "Duplizieren", "Дублировать", "Дублювати", "Duplicar", "Dupliquer", "Duplica"]),
    ("Usuń", ["Delete", "Löschen", "Удалить", "Видалити", "Eliminar", "Supprimer", "Elimina"]),
    ("Usuń na zawsze", ["Delete forever", "Endgültig löschen", "Удалить навсегда", "Видалити назавжди", "Eliminar para siempre", "Supprimer définitivement", "Elimina per sempre"]),
    ("Nie usuwaj", ["Keep it", "Behalten", "Не удалять", "Не видаляти", "No eliminar", "Ne pas supprimer", "Non eliminare"]),
    ("przed chwilą", ["just now", "gerade eben", "только что", "щойно", "ahora mismo", "à l'instant", "proprio ora"]),
    ("{} min temu", ["{} min ago", "vor {} Min.", "{} мин назад", "{} хв тому", "hace {} min", "il y a {} min", "{} min fa"]),
    ("{} godz. temu", ["{} h ago", "vor {} Std.", "{} ч назад", "{} год тому", "hace {} h", "il y a {} h", "{} h fa"]),
    ("wczoraj", ["yesterday", "gestern", "вчера", "учора", "ayer", "hier", "ieri"]),
    ("{} dni temu", ["{} days ago", "vor {} Tagen", "{} дн. назад", "{} дн. тому", "hace {} días", "il y a {} jours", "{} giorni fa"]),
    ("Rysunek", ["Drawing", "Zeichnung", "Рисунок", "Малюнок", "Dibujo", "Dessin", "Disegno"]),
    ("{} (kopia)", ["{} (copy)", "{} (Kopie)", "{} (копия)", "{} (копія)", "{} (copia)", "{} (copie)", "{} (copia)"]),
    ("Zmieniono nazwę na „{}”", ["Renamed to “{}”", "Umbenannt in „{}“", "Переименовано в «{}»", "Перейменовано на «{}»", "Renombrado a «{}»", "Renommé en « {} »", "Rinominato in «{}»"]),
    ("Utworzono kopię „{}”", ["Copy “{}” created", "Kopie „{}“ erstellt", "Создана копия «{}»", "Створено копію «{}»", "Copia «{}» creada", "Copie « {} » créée", "Copia «{}» creata"]),
    ("Nie udało się skopiować: {}", ["Could not copy: {}", "Kopieren fehlgeschlagen: {}", "Не удалось скопировать: {}", "Не вдалося скопіювати: {}", "No se pudo copiar: {}", "Copie impossible : {}", "Copia non riuscita: {}"]),
    ("Usunięto „{}”", ["Deleted “{}”", "„{}“ gelöscht", "«{}» удалён", "«{}» видалено", "«{}» eliminado", "« {} » supprimé", "«{}» eliminato"]),
    ("Nie udało się usunąć: {}", ["Could not delete: {}", "Löschen fehlgeschlagen: {}", "Не удалось удалить: {}", "Не вдалося видалити: {}", "No se pudo eliminar: {}", "Suppression impossible : {}", "Eliminazione non riuscita: {}"]),
    ("Nie udało się utworzyć kartki: {}", ["Could not create the page: {}", "Seite konnte nicht erstellt werden: {}", "Не удалось создать лист: {}", "Не вдалося створити аркуш: {}", "No se pudo crear la hoja: {}", "Impossible de créer la page : {}", "Impossibile creare il foglio: {}"]),
    ("Plik był uszkodzony: {}", ["The file was damaged: {}", "Die Datei war beschädigt: {}", "Файл был повреждён: {}", "Файл був пошкоджений: {}", "El archivo estaba dañado: {}", "Le fichier était endommagé : {}", "Il file era danneggiato: {}"]),
    ("Nieprawidłowa nazwa", ["Invalid name", "Ungültiger Name", "Недопустимое имя", "Неприпустима назва", "Nombre no válido", "Nom invalide", "Nome non valido"]),
    ("Projekt „{}” już istnieje", ["Project “{}” already exists", "Projekt „{}“ existiert bereits", "Проект «{}» уже существует", "Проєкт «{}» уже існує", "El proyecto «{}» ya existe", "Le projet « {} » existe déjà", "Il progetto «{}» esiste già"]),
    // Photos
    ("Zdjęcia", ["Photos", "Fotos", "Фото", "Фото", "Fotos", "Photos", "Foto"]),
    ("Dodaj zdjęcie", ["Add a photo", "Foto hinzufügen", "Добавить фото", "Додати фото", "Añadir foto", "Ajouter une photo", "Aggiungi foto"]),
    ("Dodane zdjęcie leży pod rysunkiem – możesz je odrysować.", ["An added photo lies under the drawing – you can trace it.", "Ein hinzugefügtes Foto liegt unter der Zeichnung – du kannst es nachzeichnen.", "Добавленное фото лежит под рисунком – его можно обвести.", "Додане фото лежить під малюнком – його можна обвести.", "La foto añadida queda bajo el dibujo: puedes calcarla.", "La photo ajoutée est sous le dessin : vous pouvez la décalquer.", "La foto aggiunta sta sotto il disegno: puoi ricalcarla."]),
    ("Zdjęcie {}", ["Photo {}", "Foto {}", "Фото {}", "Фото {}", "Foto {}", "Photo {}", "Foto {}"]),
    ("krycie", ["opacity", "Deckkraft", "непрозрачность", "непрозорість", "opacidad", "opacité", "opacità"]),
    ("Przesuń / skaluj", ["Move / scale", "Verschieben / skalieren", "Сдвиг / масштаб", "Зсув / масштаб", "Mover / escalar", "Déplacer / redimensionner", "Sposta / scala"]),
    ("Ukryj", ["Hide", "Ausblenden", "Скрыть", "Сховати", "Ocultar", "Masquer", "Nascondi"]),
    ("Pokaż", ["Show", "Einblenden", "Показать", "Показати", "Mostrar", "Afficher", "Mostra"]),
    ("Wybierz zdjęcie", ["Choose a photo", "Foto wählen", "Выберите фото", "Виберіть фото", "Elige una foto", "Choisissez une photo", "Scegli una foto"]),
    ("Wróć", ["Back", "Zurück", "Назад", "Назад", "Atrás", "Retour", "Indietro"]),
    ("Wybieranie zdjęć nie jest dostępne na tym urządzeniu.", ["Choosing photos is not available on this device.", "Fotoauswahl ist auf diesem Gerät nicht verfügbar.", "Выбор фото недоступен на этом устройстве.", "Вибір фото недоступний на цьому пристрої.", "Elegir fotos no está disponible en este dispositivo.", "Le choix de photos n'est pas disponible sur cet appareil.", "La scelta delle foto non è disponibile su questo dispositivo."]),
    ("Wczytywanie zdjęcia…", ["Loading the photo…", "Foto wird geladen…", "Загрузка фото…", "Завантаження фото…", "Cargando la foto…", "Chargement de la photo…", "Caricamento della foto…"]),
    ("Szukam zdjęć…", ["Looking for photos…", "Suche Fotos…", "Ищу фото…", "Шукаю фото…", "Buscando fotos…", "Recherche de photos…", "Cerco le foto…"]),
    ("Zezwól na dostęp do zdjęć", ["Allow access to photos", "Zugriff auf Fotos erlauben", "Разрешить доступ к фото", "Дозволити доступ до фото", "Permitir acceso a las fotos", "Autoriser l'accès aux photos", "Consenti l'accesso alle foto"]),
    ("Odśwież", ["Refresh", "Aktualisieren", "Обновить", "Оновити", "Actualizar", "Actualiser", "Aggiorna"]),
    ("Jak dodać to zdjęcie?", ["How should this photo be added?", "Wie soll das Foto hinzugefügt werden?", "Как добавить это фото?", "Як додати це фото?", "¿Cómo añadir esta foto?", "Comment ajouter cette photo ?", "Come aggiungere questa foto?"]),
    ("Do odrysowania – zdjęcie pod rysunkiem", ["To trace – the photo under the drawing", "Zum Nachzeichnen – Foto unter der Zeichnung", "Для обводки – фото под рисунком", "Для обведення – фото під малюнком", "Para calcar – la foto bajo el dibujo", "À décalquer – la photo sous le dessin", "Da ricalcare – la foto sotto il disegno"]),
    ("Zamień na rysunek (wektorowy, można go potem edytować):", ["Turn into a drawing (vector, editable afterwards):", "In eine Zeichnung umwandeln (Vektor, danach bearbeitbar):", "Превратить в рисунок (векторный, можно редактировать):", "Перетворити на малюнок (векторний, можна редагувати):", "Convertir en dibujo (vectorial, editable después):", "Transformer en dessin (vectoriel, modifiable ensuite) :", "Trasforma in disegno (vettoriale, modificabile poi):"]),
    ("Kolorowy", ["Colourful", "Farbig", "Цветной", "Кольоровий", "En color", "En couleur", "A colori"]),
    ("Same kontury", ["Outlines only", "Nur Konturen", "Только контуры", "Лише контури", "Solo contornos", "Contours seuls", "Solo contorni"]),
    ("szczegóły", ["detail", "Details", "детали", "деталі", "detalle", "détails", "dettagli"]),
    ("kontury na wierzchu (aktualnym pędzlem i kolorem)", ["outlines on top (current brush and colour)", "Konturen darüber (aktueller Pinsel und Farbe)", "контуры сверху (текущей кистью и цветом)", "контури зверху (поточним пензлем і кольором)", "contornos encima (pincel y color actuales)", "contours par-dessus (pinceau et couleur actuels)", "contorni sopra (pennello e colore attuali)"]),
    ("Kontury aktualnym pędzlem, kolorem i grubością.", ["Outlines with the current brush, colour and width.", "Konturen mit aktuellem Pinsel, Farbe und Stärke.", "Контуры текущей кистью, цветом и толщиной.", "Контури поточним пензлем, кольором і товщиною.", "Contornos con el pincel, color y grosor actuales.", "Contours avec le pinceau, la couleur et l'épaisseur actuels.", "Contorni con pennello, colore e spessore attuali."]),
    ("zostaw blade zdjęcie pod spodem", ["keep a faint photo underneath", "blasses Foto darunter lassen", "оставить бледное фото снизу", "залишити бліде фото знизу", "dejar la foto tenue debajo", "garder la photo pâle dessous", "lascia la foto sbiadita sotto"]),
    ("Zamień na rysunek", ["Turn into a drawing", "In Zeichnung umwandeln", "Превратить в рисунок", "Перетворити на малюнок", "Convertir en dibujo", "Transformer en dessin", "Trasforma in disegno"]),
    ("Anuluj", ["Cancel", "Abbrechen", "Отмена", "Скасувати", "Cancelar", "Annuler", "Annulla"]),
    ("Przesuń zdjęcie palcem, dwoma palcami skaluj i obracaj", ["Move the photo with a finger; scale and rotate with two", "Foto mit einem Finger verschieben, mit zwei skalieren und drehen", "Двигайте фото пальцем, двумя – масштаб и поворот", "Рухайте фото пальцем, двома – масштаб і поворот", "Mueve la foto con un dedo; escala y gira con dos", "Déplacez la photo d'un doigt ; redimensionnez et tournez à deux", "Sposta la foto con un dito; scala e ruota con due"]),
    ("Gotowe", ["Done", "Fertig", "Готово", "Готово", "Listo", "Terminé", "Fatto"]),
    ("Nie udało się odczytać zdjęcia", ["Could not read the photo", "Foto konnte nicht gelesen werden", "Не удалось прочитать фото", "Не вдалося прочитати фото", "No se pudo leer la foto", "Impossible de lire la photo", "Impossibile leggere la foto"]),
    ("Nie udało się wczytać zdjęcia", ["Could not load the photo", "Foto konnte nicht geladen werden", "Не удалось загрузить фото", "Не вдалося завантажити фото", "No se pudo cargar la foto", "Impossible de charger la photo", "Impossibile caricare la foto"]),
    ("Nic nie wyszło – spróbuj więcej szczegółów", ["Nothing came out – try more detail", "Nichts erkannt – mehr Details versuchen", "Ничего не вышло – попробуйте больше деталей", "Нічого не вийшло – спробуйте більше деталей", "No salió nada: prueba con más detalle", "Rien n'est sorti – essayez plus de détails", "Non è uscito nulla: prova più dettagli"]),
    ("Aplikacja potrzebuje dostępu do zdjęć.", ["The app needs access to your photos.", "Die App braucht Zugriff auf deine Fotos.", "Приложению нужен доступ к фото.", "Застосунку потрібен доступ до фото.", "La app necesita acceso a tus fotos.", "L'appli a besoin d'accéder à vos photos.", "L'app ha bisogno di accedere alle foto."]),
    ("Nie znaleziono zdjęć w galerii.", ["No photos found in the gallery.", "Keine Fotos in der Galerie gefunden.", "В галерее нет фото.", "У галереї немає фото.", "No hay fotos en la galería.", "Aucune photo dans la galerie.", "Nessuna foto nella galleria."]),
    ("Nie udało się odczytać galerii zdjęć.", ["Could not read the photo gallery.", "Fotogalerie konnte nicht gelesen werden.", "Не удалось прочитать галерею.", "Не вдалося прочитати галерею.", "No se pudo leer la galería.", "Impossible de lire la galerie.", "Impossibile leggere la galleria."]),
    ("Brak zdjęć w folderach Obrazy / Pictures / Pobrane. Możesz też przeciągnąć plik na okno.", ["No pictures in the Pictures / Downloads folders. You can also drop a file onto the window.", "Keine Bilder in Bilder / Downloads. Du kannst auch eine Datei auf das Fenster ziehen.", "Нет картинок в папках «Изображения» / «Загрузки». Можно перетащить файл в окно.", "Немає зображень у теках «Зображення» / «Завантаження». Можна перетягнути файл у вікно.", "No hay imágenes en Imágenes / Descargas. También puedes arrastrar un archivo a la ventana.", "Aucune image dans Images / Téléchargements. Vous pouvez aussi déposer un fichier sur la fenêtre.", "Nessuna immagine in Immagini / Download. Puoi anche trascinare un file sulla finestra."]),
    // Layers
    ("Warstwy", ["Layers", "Ebenen", "Слои", "Шари", "Capas", "Calques", "Livelli"]),
    ("Dodaj warstwę", ["Add a layer", "Ebene hinzufügen", "Добавить слой", "Додати шар", "Añadir capa", "Ajouter un calque", "Aggiungi livello"]),
    ("Rysujesz na zaznaczonej warstwie. Gumka ściera tylko na niej.", ["You draw on the selected layer. The eraser only erases there.", "Du zeichnest auf der gewählten Ebene. Der Radierer wirkt nur dort.", "Вы рисуете на выбранном слое. Ластик стирает только на нём.", "Ви малюєте на вибраному шарі. Гумка стирає лише на ньому.", "Dibujas en la capa seleccionada. La goma solo borra en ella.", "Vous dessinez sur le calque choisi. La gomme n'efface que sur lui.", "Disegni sul livello selezionato. La gomma cancella solo lì."]),
    ("ukryta", ["hidden", "ausgeblendet", "скрыт", "схований", "oculta", "masqué", "nascosto"]),
    ("W górę", ["Up", "Nach oben", "Вверх", "Вгору", "Subir", "Monter", "Su"]),
    ("W dół", ["Down", "Nach unten", "Вниз", "Вниз", "Bajar", "Descendre", "Giù"]),
    ("Usuń z kreskami", ["Delete with its strokes", "Mit Strichen löschen", "Удалить со штрихами", "Видалити зі штрихами", "Eliminar con sus trazos", "Supprimer avec ses traits", "Elimina con i tratti"]),
    ("Nie", ["No", "Nein", "Нет", "Ні", "No", "Non", "No"]),
    ("Warstwa {}", ["Layer {}", "Ebene {}", "Слой {}", "Шар {}", "Capa {}", "Calque {}", "Livello {}"]),
    ("Musi zostać co najmniej jedna warstwa", ["At least one layer must remain", "Mindestens eine Ebene muss bleiben", "Должен остаться хотя бы один слой", "Має лишитися хоча б один шар", "Debe quedar al menos una capa", "Il doit rester au moins un calque", "Deve restare almeno un livello"]),
    // Bookmarks and video
    ("Zakładki", ["Bookmarks", "Lesezeichen", "Закладки", "Закладки", "Marcadores", "Signets", "Segnalibri"]),
    ("Dodaj zakładkę (obecny widok)", ["Add a bookmark (current view)", "Lesezeichen hinzufügen (aktuelle Ansicht)", "Добавить закладку (текущий вид)", "Додати закладку (поточний вигляд)", "Añadir marcador (vista actual)", "Ajouter un signet (vue actuelle)", "Aggiungi segnalibro (vista attuale)"]),
    ("Wideo MP4", ["MP4 video", "MP4-Video", "Видео MP4", "Відео MP4", "Vídeo MP4", "Vidéo MP4", "Video MP4"]),
    ("Film z przelotem przez zakładki po kolei", ["A video flying through the bookmarks in order", "Ein Video, das der Reihe nach durch die Lesezeichen fliegt", "Видео с пролётом по закладкам по порядку", "Відео з прольотом по закладках по черзі", "Un vídeo que recorre los marcadores en orden", "Une vidéo qui survole les signets dans l'ordre", "Un video che vola tra i segnalibri in ordine"]),
    ("Potrzebne są co najmniej 2 zakładki", ["At least 2 bookmarks are needed", "Mindestens 2 Lesezeichen nötig", "Нужно хотя бы 2 закладки", "Потрібно хоча б 2 закладки", "Se necesitan al menos 2 marcadores", "Il faut au moins 2 signets", "Servono almeno 2 segnalibri"]),
    ("Zakładka zapamiętuje miejsce, przybliżenie i obrót. Stuknij ją, żeby tam przelecieć.", ["A bookmark remembers the place, zoom and rotation. Tap it to fly there.", "Ein Lesezeichen merkt sich Ort, Zoom und Drehung. Tippen, um dorthin zu fliegen.", "Закладка запоминает место, масштаб и поворот. Коснитесь, чтобы перелететь туда.", "Закладка запам'ятовує місце, масштаб і поворот. Торкніться, щоб перелетіти туди.", "Un marcador recuerda el lugar, el zoom y el giro. Tócalo para volar allí.", "Un signet retient le lieu, le zoom et la rotation. Touchez-le pour y voler.", "Un segnalibro ricorda posto, zoom e rotazione. Toccalo per volarci."]),
    ("OK", ["OK", "OK", "ОК", "ОК", "OK", "OK", "OK"]),
    ("Leć", ["Fly", "Fliegen", "Лететь", "Летіти", "Volar", "Aller", "Vola"]),
    ("Zakładka {}", ["Bookmark {}", "Lesezeichen {}", "Закладка {}", "Закладка {}", "Marcador {}", "Signet {}", "Segnalibro {}"]),
    ("Dodano „{}”", ["Added “{}”", "„{}“ hinzugefügt", "Добавлено «{}»", "Додано «{}»", "Añadido «{}»", "« {} » ajouté", "Aggiunto «{}»"]),
    ("Dodaj co najmniej 2 zakładki", ["Add at least 2 bookmarks", "Füge mindestens 2 Lesezeichen hinzu", "Добавьте хотя бы 2 закладки", "Додайте хоча б 2 закладки", "Añade al menos 2 marcadores", "Ajoutez au moins 2 signets", "Aggiungi almeno 2 segnalibri"]),
    ("Eksport nie jest dostępny na tym urządzeniu", ["Export is not available on this device", "Export ist auf diesem Gerät nicht verfügbar", "Экспорт недоступен на этом устройстве", "Експорт недоступний на цьому пристрої", "La exportación no está disponible en este dispositivo", "L'export n'est pas disponible sur cet appareil", "L'esportazione non è disponibile su questo dispositivo"]),
    ("Nie udało się wyrenderować obrazu", ["Could not render the picture", "Bild konnte nicht gerendert werden", "Не удалось отрисовать изображение", "Не вдалося відмалювати зображення", "No se pudo generar la imagen", "Impossible de générer l'image", "Impossibile generare l'immagine"]),
    ("Zapisywanie obrazu…", ["Saving the picture…", "Bild wird gespeichert…", "Сохранение изображения…", "Збереження зображення…", "Guardando la imagen…", "Enregistrement de l'image…", "Salvataggio dell'immagine…"]),
    ("Zapisano: {}", ["Saved: {}", "Gespeichert: {}", "Сохранено: {}", "Збережено: {}", "Guardado: {}", "Enregistré : {}", "Salvato: {}"]),
    ("Nie zapisano: {}", ["Not saved: {}", "Nicht gespeichert: {}", "Не сохранено: {}", "Не збережено: {}", "No guardado: {}", "Non enregistré : {}", "Non salvato: {}"]),
    ("Eksport wideo nie jest dostępny na tym urządzeniu", ["Video export is not available on this device", "Videoexport ist auf diesem Gerät nicht verfügbar", "Экспорт видео недоступен на этом устройстве", "Експорт відео недоступний на цьому пристрої", "La exportación de vídeo no está disponible en este dispositivo", "L'export vidéo n'est pas disponible sur cet appareil", "L'esportazione video non è disponibile su questo dispositivo"]),
    ("Zapisano wideo: {}", ["Video saved: {}", "Video gespeichert: {}", "Видео сохранено: {}", "Відео збережено: {}", "Vídeo guardado: {}", "Vidéo enregistrée : {}", "Video salvato: {}"]),
    ("Wideo nie powstało: {}", ["The video was not made: {}", "Video nicht erstellt: {}", "Видео не создано: {}", "Відео не створено: {}", "No se creó el vídeo: {}", "La vidéo n'a pas été créée : {}", "Il video non è stato creato: {}"]),
    ("Zapisywanie wideo…", ["Saving the video…", "Video wird gespeichert…", "Сохранение видео…", "Збереження відео…", "Guardando el vídeo…", "Enregistrement de la vidéo…", "Salvataggio del video…"]),
    ("Eksport wideo: {}% (Wstecz – anuluj)", ["Video export: {}% (Back – cancel)", "Videoexport: {} % (Zurück – abbrechen)", "Экспорт видео: {}% (Назад – отмена)", "Експорт відео: {}% (Назад – скасувати)", "Exportando vídeo: {} % (Atrás – cancelar)", "Export vidéo : {} % (Retour – annuler)", "Esportazione video: {}% (Indietro – annulla)"]),
    ("Eksport wideo anulowany", ["Video export cancelled", "Videoexport abgebrochen", "Экспорт видео отменён", "Експорт відео скасовано", "Exportación de vídeo cancelada", "Export vidéo annulé", "Esportazione video annullata"]),
    ("Nagrywanie nie jest dostępne na tym urządzeniu", ["Recording is not available on this device", "Aufnahme ist auf diesem Gerät nicht verfügbar", "Запись недоступна на этом устройстве", "Запис недоступний на цьому пристрої", "La grabación no está disponible en este dispositivo", "L'enregistrement n'est pas disponible sur cet appareil", "La registrazione non è disponibile su questo dispositivo"]),
    ("Zapisywanie nagrania…", ["Saving the recording…", "Aufnahme wird gespeichert…", "Сохранение записи…", "Збереження запису…", "Guardando la grabación…", "Enregistrement en cours d'écriture…", "Salvataggio della registrazione…"]),
    ("Zapisano nagranie: {}", ["Recording saved: {}", "Aufnahme gespeichert: {}", "Запись сохранена: {}", "Запис збережено: {}", "Grabación guardada: {}", "Enregistrement sauvegardé : {}", "Registrazione salvata: {}"]),
    ("Nagranie nie powstało: {}", ["The recording was not made: {}", "Aufnahme nicht erstellt: {}", "Запись не создана: {}", "Запис не створено: {}", "No se creó la grabación: {}", "L'enregistrement n'a pas été créé : {}", "La registrazione non è stata creata: {}"]),
    ("Nagrywanie przerwane: błąd kodera", ["Recording stopped: encoder error", "Aufnahme abgebrochen: Encoderfehler", "Запись прервана: ошибка кодера", "Запис перервано: помилка кодера", "Grabación detenida: error del codificador", "Enregistrement arrêté : erreur de l'encodeur", "Registrazione interrotta: errore del codificatore"]),
    ("● Nagrywanie {} – zakończ", ["● Recording {} – stop", "● Aufnahme {} – beenden", "● Запись {} – стоп", "● Запис {} – стоп", "● Grabando {} – parar", "● Enregistrement {} – arrêter", "● Registrazione {} – ferma"]),
    ("Galeria → {}", ["Gallery → {}", "Galerie → {}", "Галерея → {}", "Галерея → {}", "Galería → {}", "Galerie → {}", "Galleria → {}"]),
    ("Nie udało się zapisać obrazu", ["Could not save the picture", "Bild konnte nicht gespeichert werden", "Не удалось сохранить изображение", "Не вдалося зберегти зображення", "No se pudo guardar la imagen", "Impossible d'enregistrer l'image", "Impossibile salvare l'immagine"]),
    ("Nie udało się zakodować obrazu", ["Could not encode the picture", "Bild konnte nicht kodiert werden", "Не удалось закодировать изображение", "Не вдалося закодувати зображення", "No se pudo codificar la imagen", "Impossible d'encoder l'image", "Impossibile codificare l'immagine"]),
    // Selection, text, fill
    ("Zaznaczone: {}", ["Selected: {}", "Ausgewählt: {}", "Выделено: {}", "Виділено: {}", "Seleccionados: {}", "Sélection : {}", "Selezionati: {}"]),
    ("Kolor", ["Colour", "Farbe", "Цвет", "Колір", "Color", "Couleur", "Colore"]),
    ("Nadaj zaznaczonym kreskom aktualny kolor", ["Give the selected strokes the current colour", "Ausgewählten Strichen die aktuelle Farbe geben", "Окрасить выделенные штрихи текущим цветом", "Пофарбувати виділені штрихи поточним кольором", "Dar el color actual a los trazos seleccionados", "Donner la couleur actuelle aux traits choisis", "Dai il colore attuale ai tratti selezionati"]),
    ("Nic nie zaznaczono – obrysuj kreski", ["Nothing selected – circle some strokes", "Nichts ausgewählt – Striche umkreisen", "Ничего не выделено – обведите штрихи", "Нічого не виділено – обведіть штрихи", "Nada seleccionado: rodea algunos trazos", "Rien de sélectionné – entourez des traits", "Nessuna selezione: cerchia dei tratti"]),
    ("Wpisz tekst…", ["Type some text…", "Text eingeben…", "Введите текст…", "Введіть текст…", "Escribe un texto…", "Tapez un texte…", "Scrivi un testo…"]),
    ("Dodaj", ["Add", "Hinzufügen", "Добавить", "Додати", "Añadir", "Ajouter", "Aggiungi"]),
    ("Tekst jest wektorowy: ostry przy każdym zoomie, w aktualnym kolorze.", ["The text is vector: sharp at any zoom, in the current colour.", "Der Text ist vektoriell: scharf bei jedem Zoom, in der aktuellen Farbe.", "Текст векторный: чёткий при любом масштабе, текущего цвета.", "Текст векторний: чіткий за будь-якого масштабу, поточного кольору.", "El texto es vectorial: nítido con cualquier zoom, en el color actual.", "Le texte est vectoriel : net à tout zoom, dans la couleur actuelle.", "Il testo è vettoriale: nitido a ogni zoom, nel colore attuale."]),
    ("Nie udało się dodać tekstu", ["Could not add the text", "Text konnte nicht hinzugefügt werden", "Не удалось добавить текст", "Не вдалося додати текст", "No se pudo añadir el texto", "Impossible d'ajouter le texte", "Impossibile aggiungere il testo"]),
    ("Tu nie ma zamkniętego obszaru – obrysuj go albo przeciągnij, by narysować kształt", ["There is no closed area here – outline it, or drag to draw a shape", "Hier ist kein geschlossener Bereich – umranden oder ziehen, um eine Form zu zeichnen", "Здесь нет замкнутой области – обведите её или проведите, чтобы нарисовать фигуру", "Тут немає замкненої області – обведіть її або проведіть, щоб намалювати фігуру", "Aquí no hay un área cerrada: rodéala o arrastra para dibujar una forma", "Pas de zone fermée ici – entourez-la ou glissez pour dessiner une forme", "Qui non c'è un'area chiusa: contornala o trascina per disegnare una forma"]),
    ("log2 zoomu: {}\nkresek: {}", ["log2 zoom: {}\nstrokes: {}", "log2 Zoom: {}\nStriche: {}", "log2 масштаба: {}\nштрихов: {}", "log2 масштабу: {}\nштрихів: {}", "log2 del zoom: {}\ntrazos: {}", "log2 du zoom : {}\ntraits : {}", "log2 dello zoom: {}\ntratti: {}"]),
    // First run and settings
    ("Ustawienia", ["Settings", "Einstellungen", "Настройки", "Налаштування", "Ajustes", "Réglages", "Impostazioni"]),
    ("Język", ["Language", "Sprache", "Язык", "Мова", "Idioma", "Langue", "Lingua"]),
    ("Urządzenie", ["Device", "Gerät", "Устройство", "Пристрій", "Dispositivo", "Appareil", "Dispositivo"]),
    ("Telefon", ["Phone", "Telefon", "Телефон", "Телефон", "Teléfono", "Téléphone", "Telefono"]),
    ("Tablet", ["Tablet", "Tablet", "Планшет", "Планшет", "Tableta", "Tablette", "Tablet"]),
    ("Witaj!", ["Welcome!", "Willkommen!", "Добро пожаловать!", "Вітаємо!", "¡Bienvenido!", "Bienvenue !", "Benvenuto!"]),
    ("Wybierz język i urządzenie. Możesz to zmienić później w Więcej → Ustawienia.", ["Choose your language and device. You can change this later in More → Settings.", "Wähle Sprache und Gerät. Später änderbar unter Mehr → Einstellungen.", "Выберите язык и устройство. Позже можно изменить в «Ещё → Настройки».", "Виберіть мову та пристрій. Пізніше можна змінити в «Ще → Налаштування».", "Elige idioma y dispositivo. Puedes cambiarlo luego en Más → Ajustes.", "Choisissez la langue et l'appareil. Modifiable ensuite dans Plus → Réglages.", "Scegli lingua e dispositivo. Puoi cambiarli poi in Altro → Impostazioni."]),
    ("Telefon: narzędzia w kilku rzędach, wygodne na wąskim ekranie.", ["Phone: tools in several rows, handy on a narrow screen.", "Telefon: Werkzeuge in mehreren Reihen, praktisch auf schmalem Bildschirm.", "Телефон: инструменты в несколько рядов, удобно на узком экране.", "Телефон: інструменти в кілька рядів, зручно на вузькому екрані.", "Teléfono: herramientas en varias filas, cómodo en pantalla estrecha.", "Téléphone : outils sur plusieurs rangées, pratique sur écran étroit.", "Telefono: strumenti su più righe, comodo su schermo stretto."]),
    ("Tablet: większe przyciski i tekst, narzędzia w jednym rzędzie na szerokim ekranie.", ["Tablet: bigger buttons and text, tools in one row on a wide screen.", "Tablet: größere Knöpfe und Schrift, Werkzeuge in einer Reihe auf breitem Bildschirm.", "Планшет: крупнее кнопки и текст, инструменты в один ряд на широком экране.", "Планшет: більші кнопки й текст, інструменти в один ряд на широкому екрані.", "Tableta: botones y texto más grandes, herramientas en una fila en pantalla ancha.", "Tablette : boutons et texte plus grands, outils sur une rangée sur écran large.", "Tablet: pulsanti e testo più grandi, strumenti su una riga su schermo largo."]),
    ("Zaczynamy", ["Let's start", "Los geht's", "Начать", "Почати", "Empezar", "Commencer", "Iniziamo"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_fallback_and_placeholders() {
        // (No global language changes here: other tests run in parallel.)
        assert_eq!(t_in(Lang::De, "Gumka"), "Radierer");
        assert_eq!(t_in(Lang::De, "nie ma takiego klucza"), "nie ma takiego klucza", "missing → Polish");
        assert_eq!(tf_in(Lang::De, "Zaznaczone: {}", &[&3]), "Ausgewählt: 3");
        assert_eq!(tf_in(Lang::Ru, "{} min temu", &[&5]), "5 мин назад");
        assert_eq!(tf_in(Lang::Pl, "Warstwa {}", &[&2]), "Warstwa 2");
        assert_eq!(Lang::from_code("uk_UA.UTF-8"), Lang::Uk);
        assert_eq!(Lang::from_code("ja"), Lang::En);
    }

    #[test]
    fn table_is_consistent() {
        // Every translation keeps the same number of placeholders as its key, no key twice.
        let mut seen = std::collections::HashSet::new();
        for (k, row) in TABLE {
            assert!(seen.insert(*k), "duplicate key {k}");
            let n = k.matches("{}").count();
            for v in row {
                assert_eq!(v.matches("{}").count(), n, "{k} → {v}");
                assert!(!v.is_empty(), "{k}");
            }
        }
    }
}
