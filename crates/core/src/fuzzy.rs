//! Recherche approximative (palette de commandes) : les lettres de la
//! requête doivent apparaître dans l'ordre, sans tenir compte de la casse ni
//! des accents. Le score favorise les correspondances au début du texte et
//! des mots, et les lettres consécutives.

/// Minuscule sans accent (lettres françaises courantes).
pub fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' => 'i',
        'ô' | 'ö' | 'ó' | 'õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'ÿ' => 'y',
        'œ' => 'o',
        'æ' => 'a',
        _ => c,
    }
}

/// Score de `text` pour `query` (plus grand = meilleur), `None` si les
/// lettres de la requête n'y apparaissent pas dans l'ordre. Une requête
/// vide correspond à tout (score 0). Les espaces de la requête sont ignorés.
pub fn score(query: &str, text: &str) -> Option<i64> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().map(fold).collect();
    let mut s: i64 = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (ti, &c) in t.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c != q[qi] {
            continue;
        }
        let word_start = ti == 0 || !t[ti - 1].is_alphanumeric();
        s += 1;
        if ti == 0 {
            s += 8;
        } else if word_start {
            s += 5;
        }
        if prev.is_some_and(|p| p + 1 == ti) {
            s += 4;
        }
        prev = Some(ti);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    // Les textes courts l'emportent à égalité.
    Some(s * 100 - t.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_ignoring_case_and_accents() {
        assert!(score("theme", "Thème sombre").is_some());
        assert!(score("ts", "Thème sombre").is_some());
        assert!(score("hs", "Thème sombre").is_some());
        assert!(score("xyz", "Thème sombre").is_none());
        assert!(
            score("sombreth", "Thème sombre").is_none(),
            "ordre respecté"
        );
        assert_eq!(score("  ", "abc"), Some(0));
    }

    #[test]
    fn ranking() {
        fn rank<'a>(q: &str, items: &[&'a str]) -> Vec<&'a str> {
            let mut v: Vec<(&'a str, i64)> = items
                .iter()
                .filter_map(|t| score(q, t).map(|s| (*t, s)))
                .collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.1));
            v.into_iter().map(|(t, _)| t).collect()
        }
        assert_eq!(
            rank("gra", &["Exporter le graphe", "Graphe 3D", "Paragraphe"]),
            ["Graphe 3D", "Exporter le graphe", "Paragraphe"]
        );
        assert_eq!(
            rank("nf", &["Nouvelle fiche", "Info"]),
            ["Nouvelle fiche", "Info"]
        );
    }
}
