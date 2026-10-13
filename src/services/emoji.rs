//! Launcher emoji picker (`:fuego`, `:fire`, `:thumbs up`). Every Unicode
//! emoji comes from the `emojis` crate (CLDR names + GitHub shortcodes); the
//! table below adds Spanish keywords for the popular ones and doubles as the
//! "most used" order shown for an empty query and used to break ties.

use std::sync::OnceLock;

/// Popular emoji first, with Spanish keywords (accents optional when typing).
const POPULAR: &[(&str, &str)] = &[
    ("😂", "risa llorar carcajada jaja"),
    ("❤️", "corazon amor rojo"),
    ("👍", "pulgar arriba bien ok vale like"),
    ("😊", "sonrisa feliz contento"),
    ("🙏", "gracias por favor rezar manos"),
    ("😭", "llorar llanto triste"),
    ("🔥", "fuego llama caliente"),
    ("😍", "enamorado ojos corazon amor"),
    ("🥰", "carino amor corazones"),
    ("😘", "beso"),
    ("🤣", "revolcarse risa jaja"),
    ("😅", "sudor risa nervios"),
    ("😁", "sonrisa dientes"),
    ("😉", "guino"),
    ("😎", "gafas sol genial"),
    ("🤔", "pensar pensando duda"),
    ("🙄", "ojos blanco fastidio"),
    ("😢", "lagrima triste"),
    ("😡", "enojado furioso rabia"),
    ("😱", "grito miedo susto"),
    ("😴", "dormir sueno zzz"),
    ("🤗", "abrazo"),
    ("🤩", "estrellas asombro"),
    ("😇", "angel inocente"),
    ("🥲", "sonrisa lagrima"),
    ("😬", "mueca incomodo"),
    ("🤯", "explota cabeza mente"),
    ("🥳", "fiesta celebrar cumpleanos"),
    ("😏", "picaro sonrisa ladeada"),
    ("😳", "sonrojado avergonzado"),
    ("🤤", "babear"),
    ("🤢", "nauseas asco"),
    ("🤮", "vomitar asco"),
    ("🤒", "enfermo fiebre"),
    ("😷", "mascarilla enfermo"),
    ("🥺", "suplicar ojos tiernos"),
    ("😐", "neutral serio"),
    ("😶", "sin boca callado"),
    ("🫠", "derretir"),
    ("🤡", "payaso"),
    ("💀", "calavera muerto"),
    ("👻", "fantasma"),
    ("👽", "alien extraterrestre"),
    ("🤖", "robot"),
    ("💩", "caca popo"),
    ("👏", "aplausos aplaudir"),
    ("🙌", "celebrar manos arriba"),
    ("👌", "ok perfecto"),
    ("✌️", "paz victoria"),
    ("🤞", "dedos cruzados suerte"),
    ("👋", "hola adios saludo mano"),
    ("💪", "fuerza musculo biceps"),
    ("👎", "pulgar abajo mal"),
    ("👀", "ojos mirar"),
    ("🫶", "manos corazon"),
    ("🤝", "acuerdo trato apreton manos"),
    ("✍️", "escribir"),
    ("👉", "senalar derecha"),
    ("👈", "senalar izquierda"),
    ("☝️", "senalar arriba"),
    ("🖐️", "mano"),
    ("💯", "cien perfecto"),
    ("✨", "brillos chispas magia"),
    ("⭐", "estrella"),
    ("🌟", "estrella brillante"),
    ("💥", "explosion choque"),
    ("💫", "mareo estrella"),
    ("🎉", "fiesta celebrar confeti"),
    ("🎊", "confeti fiesta"),
    ("🎁", "regalo"),
    ("🎂", "pastel tarta cumpleanos"),
    ("🎈", "globo"),
    ("🏆", "trofeo ganador copa"),
    ("🥇", "medalla oro primero"),
    ("✅", "hecho check correcto si"),
    ("❌", "cruz no error mal"),
    ("⚠️", "advertencia aviso peligro"),
    ("❓", "pregunta interrogacion"),
    ("❗", "exclamacion importante"),
    ("💡", "idea bombilla"),
    ("📌", "chincheta fijar"),
    ("📎", "clip"),
    ("🔗", "enlace link cadena"),
    ("🔒", "candado cerrado seguro"),
    ("🔑", "llave clave"),
    ("⏰", "alarma reloj despertador"),
    ("⌛", "reloj arena tiempo"),
    ("📅", "calendario fecha"),
    ("📝", "nota escribir memo"),
    ("📚", "libros estudiar"),
    ("💻", "portatil ordenador computadora"),
    ("🖥️", "ordenador computadora pantalla"),
    ("⌨️", "teclado"),
    ("🖱️", "raton mouse"),
    ("📱", "movil celular telefono"),
    ("📷", "camara foto"),
    ("🎮", "videojuego mando juego"),
    ("🎧", "auriculares musica"),
    ("🎵", "musica nota"),
    ("🎶", "musica notas"),
    ("🎬", "cine pelicula claqueta"),
    ("📺", "television tele"),
    ("🔔", "campana notificacion"),
    ("🔕", "silencio campana"),
    ("📣", "megafono anuncio"),
    ("💬", "mensaje chat bocadillo"),
    ("📧", "correo email"),
    ("📦", "paquete caja"),
    ("🛒", "carrito compra"),
    ("💰", "dinero bolsa"),
    ("💸", "dinero volando gastar"),
    ("💳", "tarjeta credito"),
    ("📈", "grafico sube subir"),
    ("📉", "grafico baja bajar"),
    ("🚀", "cohete lanzamiento"),
    ("✈️", "avion viaje"),
    ("🚗", "coche carro auto"),
    ("🚲", "bicicleta bici"),
    ("🏠", "casa hogar"),
    ("🏢", "oficina edificio"),
    ("🌍", "mundo tierra planeta"),
    ("🗺️", "mapa"),
    ("⛰️", "montana"),
    ("🏖️", "playa vacaciones"),
    ("☀️", "sol soleado"),
    ("🌙", "luna noche"),
    ("⛅", "nublado sol nube"),
    ("☁️", "nube"),
    ("🌧️", "lluvia"),
    ("⛈️", "tormenta rayo"),
    ("❄️", "nieve copo frio"),
    ("🌈", "arcoiris"),
    ("⚡", "rayo electricidad energia"),
    ("💧", "gota agua"),
    ("🌊", "ola mar"),
    ("🌸", "flor cerezo"),
    ("🌹", "rosa flor"),
    ("🌻", "girasol flor"),
    ("🌱", "planta brote"),
    ("🌳", "arbol"),
    ("🍀", "trebol suerte"),
    ("🍁", "hoja arce otono"),
    ("🐶", "perro"),
    ("🐱", "gato"),
    ("🐭", "raton"),
    ("🐰", "conejo"),
    ("🦊", "zorro"),
    ("🐻", "oso"),
    ("🐼", "panda"),
    ("🐸", "rana"),
    ("🐵", "mono"),
    ("🐔", "gallina pollo"),
    ("🐧", "pinguino"),
    ("🐦", "pajaro"),
    ("🦋", "mariposa"),
    ("🐝", "abeja"),
    ("🐢", "tortuga"),
    ("🐍", "serpiente"),
    ("🐟", "pez"),
    ("🦄", "unicornio"),
    ("🍎", "manzana"),
    ("🍌", "platano banana"),
    ("🍓", "fresa"),
    ("🍉", "sandia"),
    ("🍋", "limon"),
    ("🍕", "pizza"),
    ("🍔", "hamburguesa"),
    ("🍟", "papas patatas fritas"),
    ("🌮", "taco"),
    ("🍣", "sushi"),
    ("🍜", "fideos ramen sopa"),
    ("🍰", "pastel tarta"),
    ("🍪", "galleta"),
    ("🍫", "chocolate"),
    ("🍿", "palomitas"),
    ("☕", "cafe"),
    ("🍵", "te"),
    ("🍺", "cerveza"),
    ("🍷", "vino"),
    ("🥂", "brindis copas"),
    ("⚽", "futbol balon"),
    ("🏀", "baloncesto basquet"),
    ("🎯", "diana objetivo"),
    ("🎨", "arte pintura paleta"),
    ("🧠", "cerebro mente"),
    ("❤️‍🔥", "corazon fuego"),
    ("💔", "corazon roto"),
    ("🧡", "corazon naranja"),
    ("💛", "corazon amarillo"),
    ("💚", "corazon verde"),
    ("💙", "corazon azul"),
    ("💜", "corazon morado"),
    ("🖤", "corazon negro"),
    ("🤍", "corazon blanco"),
    ("💕", "corazones amor"),
    ("👑", "corona rey reina"),
    ("💎", "diamante joya"),
    ("🎤", "microfono cantar"),
    ("🔧", "llave inglesa herramienta"),
    ("🛠️", "herramientas"),
    ("⚙️", "engranaje ajustes configuracion"),
    ("🐛", "bicho error bug"),
    ("🧪", "probeta prueba"),
    ("🗑️", "papelera basura"),
    ("🔍", "lupa buscar"),
    ("➕", "mas sumar"),
    ("➖", "menos restar"),
    ("➡️", "flecha derecha"),
    ("⬅️", "flecha izquierda"),
    ("⬆️", "flecha arriba"),
    ("⬇️", "flecha abajo"),
    ("🔄", "recargar repetir"),
    ("🆗", "ok"),
    ("🆕", "nuevo"),
    ("🇪🇸", "espana bandera"),
    ("🇲🇽", "mexico bandera"),
    ("🇨🇷", "costa rica bandera"),
    ("🇦🇷", "argentina bandera"),
    ("🇨🇴", "colombia bandera"),
];

#[derive(Debug)]
pub struct Entry {
    pub emoji: &'static str,
    pub name: &'static str,
    /// GitHub shortcodes, e.g. `["fire"]`.
    pub shortcodes: Vec<&'static str>,
    /// Folded search words (English name, shortcodes, Spanish keywords).
    words: Vec<String>,
    /// Folded Spanish keywords only (to name the emoji in Spanish).
    pub es: Option<&'static str>,
    rank: usize,
}

fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut entries: Vec<Entry> = emojis::iter()
            .enumerate()
            .map(|(i, e)| {
                let es = POPULAR.iter().position(|(p, _)| *p == e.as_str());
                let shortcodes: Vec<&'static str> = e.shortcodes().collect();
                let mut words: Vec<String> = e.name().split([' ', '-', ':']).map(fold).collect();
                words.extend(shortcodes.iter().flat_map(|s| s.split('_')).map(fold));
                if let Some(p) = es {
                    words.extend(POPULAR[p].1.split_whitespace().map(fold));
                }
                words.retain(|w| !w.is_empty());
                words.dedup();
                Entry { emoji: e.as_str(), name: e.name(), shortcodes, words, es: es.map(|p| POPULAR[p].1), rank: es.unwrap_or(POPULAR.len() + i) }
            })
            .collect();
        entries.sort_by_key(|e| e.rank);
        entries
    })
}

/// Best matches for `query` (every query word must match the start of a
/// keyword); an empty query lists the popular ones.
pub fn search(query: &str, limit: usize) -> Vec<&'static Entry> {
    let terms: Vec<String> = query.split_whitespace().map(fold).filter(|t| !t.is_empty()).collect();
    let all = index();
    if terms.is_empty() {
        return all.iter().take(limit.min(POPULAR.len())).collect();
    }
    let mut scored: Vec<(u32, &Entry)> = all
        .iter()
        .filter_map(|e| {
            let mut total = 0;
            for t in &terms {
                let best = e
                    .words
                    .iter()
                    .map(|w| if w == t { 3 } else if w.starts_with(t.as_str()) { 2 } else if t.len() >= 3 && w.contains(t.as_str()) { 1 } else { 0 })
                    .max()
                    .unwrap_or(0);
                if best == 0 {
                    return None;
                }
                total += best;
            }
            Some((total, e))
        })
        .collect();
    // Higher score first; ties keep the popularity order (stable sort).
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

impl Entry {
    /// Display name in the UI language ("fuego" / "fire").
    pub fn label(&self, es: bool) -> String {
        let name = match (es, self.es) {
            (true, Some(k)) => k.split_whitespace().next().unwrap_or(self.name),
            _ => self.name,
        };
        crate::i18n::capitalize(name)
    }
}

/// Lowercase without diacritics, so "corazón" matches "corazon" and vice versa.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(q: &str) -> &'static str {
        search(q, 5).first().map(|e| e.emoji).unwrap_or_default()
    }

    #[test]
    fn finds_by_spanish_and_english_keywords() {
        assert_eq!(first("fuego"), "🔥");
        assert_eq!(first("fire"), "🔥");
        assert_eq!(first("fue"), "🔥");
        assert_eq!(first("corazón roto"), "💔");
        assert_eq!(first("corazon roto"), "💔");
        assert_eq!(first("thumbs up"), "👍");
        assert_eq!(first("cohete"), "🚀");
        assert_eq!(first("rocket"), "🚀");
    }

    #[test]
    fn popular_entries_exist_in_the_unicode_set() {
        // A typo in the table would silently drop its Spanish keywords.
        let missing: Vec<&str> = POPULAR.iter().map(|(e, _)| *e).filter(|e| emojis::get(e).is_none()).collect();
        assert!(missing.is_empty(), "not found in the emoji set: {missing:?}");
        let dupes = POPULAR.iter().enumerate().filter(|(i, (e, _))| POPULAR[..*i].iter().any(|(o, _)| o == e)).count();
        assert_eq!(dupes, 0);
    }

    #[test]
    fn empty_query_lists_popular_and_unknown_matches_nothing() {
        let top: Vec<&str> = search("", 3).iter().map(|e| e.emoji).collect();
        assert_eq!(top, ["😂", "❤️", "👍"]);
        assert!(search("zzzxqv", 10).is_empty());
        assert!(search("fuego", 1000).len() < 50);
    }

    #[test]
    fn labels_follow_the_language() {
        let fire = search("fuego", 1)[0];
        assert_eq!(fire.label(true), "Fuego");
        assert_eq!(fire.label(false), "Fire");
        assert!(fire.shortcodes.contains(&"fire"));
    }

    #[test]
    fn folds_accents() {
        assert_eq!(fold("Corazón"), "corazon");
        assert_eq!(fold("España!"), "espana");
    }
}
