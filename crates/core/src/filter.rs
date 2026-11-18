//! Filtres combinables de la liste des fiches (et plus tard du graphe).
//!
//! Les critères se combinent par ET ; à l'intérieur d'un critère à plusieurs
//! valeurs (statuts, types de liens), c'est un OU. Pour les catégories
//! incluses, le mode est au choix : toutes (ET) ou au moins une (OU).

/// Combinaison des catégories incluses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CategoryMode {
    /// La fiche doit avoir toutes les catégories incluses.
    #[default]
    All,
    /// La fiche doit avoir au moins une des catégories incluses.
    Any,
}

/// Filtre de fiches. La valeur par défaut ne filtre rien.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NoteFilter {
    /// Texte recherché dans le titre (insensible à la casse).
    pub text: String,
    /// Chercher aussi le texte dans le corps.
    pub in_body: bool,
    pub include_categories: Vec<String>,
    pub category_mode: CategoryMode,
    /// La fiche ne doit avoir aucune de ces catégories.
    pub exclude_categories: Vec<String>,
    /// Au moins un de ces statuts (`None` : sans statut).
    pub statuses: Vec<Option<String>>,
    /// Au moins un lien (sortant ou entrant) de l'un de ces types
    /// (`None` : lien sans type).
    pub link_types: Vec<Option<String>>,
    /// Seulement les fiches qui ont des liens cassés.
    pub broken_only: bool,
}

impl NoteFilter {
    /// Nombre de critères actifs hors texte (pour l'affichage).
    pub fn active_count(&self) -> usize {
        usize::from(!self.include_categories.is_empty())
            + usize::from(!self.exclude_categories.is_empty())
            + usize::from(!self.statuses.is_empty())
            + usize::from(!self.link_types.is_empty())
            + usize::from(self.broken_only)
    }

    /// Le résultat dépend-il du contenu des fiches (corps, métadonnées,
    /// liens) et pas seulement de leurs titres ?
    pub fn depends_on_content(&self) -> bool {
        self.active_count() > 0 || (self.in_body && !self.text.trim().is_empty())
    }

    /// Remet à zéro tous les critères sauf le texte.
    pub fn clear_criteria(&mut self) {
        *self = NoteFilter {
            text: std::mem::take(&mut self.text),
            in_body: self.in_body,
            ..NoteFilter::default()
        };
    }

    /// Bascule une catégorie : neutre → incluse → exclue → neutre.
    pub fn cycle_category(&mut self, id: &str) {
        if let Some(i) = self.include_categories.iter().position(|c| c == id) {
            self.include_categories.remove(i);
            self.exclude_categories.push(id.to_owned());
        } else if let Some(i) = self.exclude_categories.iter().position(|c| c == id) {
            self.exclude_categories.remove(i);
        } else {
            self.include_categories.push(id.to_owned());
        }
    }

    /// Ajoute ou retire une valeur d'une liste de critères.
    pub fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
        if let Some(i) = list.iter().position(|v| *v == value) {
            list.remove(i);
        } else {
            list.push(value);
        }
    }

    /// Les catégories d'une fiche passent-elles le filtre ?
    pub fn matches_categories(&self, categories: &[String]) -> bool {
        let has = |c: &String| categories.contains(c);
        let included = match self.category_mode {
            _ if self.include_categories.is_empty() => true,
            CategoryMode::All => self.include_categories.iter().all(has),
            CategoryMode::Any => self.include_categories.iter().any(has),
        };
        included && !self.exclude_categories.iter().any(has)
    }

    pub fn matches_status(&self, status: Option<&str>) -> bool {
        self.statuses.is_empty() || self.statuses.iter().any(|s| s.as_deref() == status)
    }
}

/// Groupe de la liste triée par catégorie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryGroup {
    /// Catégorie (`None` : fiches sans catégorie).
    pub category: Option<String>,
    pub notes: Vec<crate::NoteId>,
}

/// Range des fiches par catégorie : une fiche à plusieurs catégories
/// apparaît dans chacune. Ordre : catégories du pack (dans leur ordre),
/// puis catégories inconnues du pack (alphabétique), puis « sans
/// catégorie ». L'ordre des fiches de `ids` est conservé dans chaque
/// groupe ; les groupes vides sont omis.
pub fn group_by_category(
    ids: &[crate::NoteId],
    categories_of: impl Fn(&crate::NoteId) -> Vec<String>,
    pack_order: &[String],
) -> Vec<CategoryGroup> {
    use std::collections::BTreeMap;
    let mut known: Vec<Vec<crate::NoteId>> = vec![Vec::new(); pack_order.len()];
    let mut unknown: BTreeMap<String, Vec<crate::NoteId>> = BTreeMap::new();
    let mut none = Vec::new();
    for id in ids {
        let cats = categories_of(id);
        if cats.is_empty() {
            none.push(id.clone());
        }
        let mut seen: Vec<&String> = Vec::new();
        for c in &cats {
            if seen.contains(&c) {
                continue;
            }
            seen.push(c);
            match pack_order.iter().position(|p| p == c) {
                Some(i) => known[i].push(id.clone()),
                None => unknown.entry(c.clone()).or_default().push(id.clone()),
            }
        }
    }
    let mut out: Vec<CategoryGroup> = pack_order
        .iter()
        .zip(known)
        .filter(|(_, n)| !n.is_empty())
        .map(|(c, notes)| CategoryGroup {
            category: Some(c.clone()),
            notes,
        })
        .collect();
    out.extend(unknown.into_iter().map(|(c, notes)| CategoryGroup {
        category: Some(c),
        notes,
    }));
    if !none.is_empty() {
        out.push(CategoryGroup {
            category: None,
            notes: none,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_by_category_with_duplicates() {
        use crate::NoteId;
        let id = |s: &str| NoteId::parse(s).unwrap();
        let ids = [id("A"), id("B"), id("C"), id("D")];
        let cats = |n: &NoteId| -> Vec<String> {
            match n.as_str() {
                "A" => vec!["piece".into(), "code".into()],
                "B" => vec!["code".into(), "code".into()],
                "C" => vec!["zzz".into()],
                _ => vec![],
            }
        };
        let order = ["piece".to_owned(), "indice".to_owned(), "code".to_owned()];
        let g = super::group_by_category(&ids, cats, &order);
        let summary: Vec<(Option<&str>, Vec<&str>)> = g
            .iter()
            .map(|g| {
                (
                    g.category.as_deref(),
                    g.notes.iter().map(NoteId::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some("piece"), vec!["A"]),
                (Some("code"), vec!["A", "B"]),
                (Some("zzz"), vec!["C"]),
                (None, vec!["D"]),
            ]
        );
    }

    use super::*;

    fn cats(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn default_matches_everything() {
        let f = NoteFilter::default();
        assert!(f.matches_categories(&[]));
        assert!(f.matches_status(None));
        assert_eq!(f.active_count(), 0);
        assert!(!f.depends_on_content());
    }

    #[test]
    fn categories_all_any_and_exclusion() {
        let mut f = NoteFilter::default();
        f.cycle_category("piece");
        f.cycle_category("indice");
        assert!(f.matches_categories(&cats(&["piece", "indice", "objet"])));
        assert!(!f.matches_categories(&cats(&["piece"])));
        f.category_mode = CategoryMode::Any;
        assert!(f.matches_categories(&cats(&["piece"])));
        assert!(!f.matches_categories(&cats(&["objet"])));
        // Deuxième clic : la catégorie passe en exclusion.
        f.cycle_category("indice");
        assert_eq!(f.exclude_categories, ["indice"]);
        assert!(!f.matches_categories(&cats(&["piece", "indice"])));
        assert!(f.matches_categories(&cats(&["piece"])));
        // Troisième clic : neutre.
        f.cycle_category("indice");
        assert!(f.exclude_categories.is_empty());
    }

    #[test]
    fn statuses_are_or_with_none() {
        let mut f = NoteFilter::default();
        NoteFilter::toggle(&mut f.statuses, Some("theorie".to_owned()));
        NoteFilter::toggle(&mut f.statuses, None);
        assert!(f.matches_status(Some("theorie")));
        assert!(f.matches_status(None));
        assert!(!f.matches_status(Some("resolu")));
        NoteFilter::toggle(&mut f.statuses, None);
        assert!(!f.matches_status(None));
    }

    #[test]
    fn clear_keeps_text() {
        let mut f = NoteFilter {
            text: "cof".into(),
            broken_only: true,
            ..Default::default()
        };
        f.cycle_category("piece");
        assert_eq!(f.active_count(), 2);
        f.clear_criteria();
        assert_eq!(f.text, "cof");
        assert_eq!(f.active_count(), 0);
    }
}
