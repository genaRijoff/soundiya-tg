//! "Алиса, передай Маше, что я буду через час": who the phrase is for and what to pass on.
//! soundiya hands over the rest of the phrase ("Маше, что я буду через час"); the first word
//! names the person in any case ending (Маше, Машуле, маме), the rest is the message.

pub struct Person {
    pub id: i64,
    /// As the bot knows it: a listener's name or an owner's Telegram first name.
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Miss {
    /// Nobody's name looks like this word.
    Unknown(String),
    /// Two or more names look like it equally well.
    Ambiguous,
    /// A name but nothing to pass on.
    Empty,
}

/// Lower case letters only, ё as е: "Маша🌸" -> "маша".
fn simplify(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

/// How well a spoken word names a person: the common start of the two, if it is long enough
/// to be the same name in another case ("маше" / "машуля": "маш").
fn closeness(word: &str, name: &str) -> Option<usize> {
    let common = word
        .chars()
        .zip(name.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let shorter = word.chars().count().min(name.chars().count());
    // Russian case endings change up to the last two letters; very short names must match whole.
    (common >= 3.min(shorter) && common + 2 >= shorter && common > 0).then_some(common)
}

/// The person (index into `people`) and the words to pass on.
pub fn address(rest: &str, people: &[Person]) -> Result<(usize, String), Miss> {
    let mut words = rest.split_whitespace();
    let spoken = simplify(words.next().unwrap_or(""));
    let mut best: Vec<(usize, usize)> = people
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let name = simplify(p.name.split_whitespace().next().unwrap_or(""));
            closeness(&spoken, &name).map(|score| (i, score))
        })
        .collect();
    best.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    let index = match best.as_slice() {
        [] => return Err(Miss::Unknown(spoken)),
        [(a, s), (_, t), ..] if s == t && people[*a].id != people[best[1].0].id => {
            return Err(Miss::Ambiguous);
        }
        [(i, _), ..] => *i,
    };
    // "Маше, что я буду через час" -> "я буду через час".
    let mut text: Vec<&str> = words.collect();
    while text
        .first()
        .is_some_and(|w| matches!(simplify(w).as_str(), "что" | ""))
    {
        text.remove(0);
    }
    let text = text.join(" ");
    let text = text.trim_matches(|c: char| c.is_whitespace() || ",:;-—".contains(c));
    if text.is_empty() {
        return Err(Miss::Empty);
    }
    Ok((index, text.to_owned()))
}

/// What Alice says back; plain words only (no emoji or markup for the voice).
pub fn spoken_name(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> Vec<Person> {
        [
            (77, "Маша🌸"),
            (42, "Сергей"),
            (88, "Мама"),
            (99, "Дима К."),
        ]
        .into_iter()
        .map(|(id, name)| Person {
            id,
            name: name.into(),
        })
        .collect()
    }

    #[test]
    fn names_are_found_in_any_case_and_the_rest_is_the_message() {
        let p = people();
        let to = |rest: &str| address(rest, &p).map(|(i, text)| (p[i].id, text));
        assert_eq!(
            to("Маше, что я буду через час"),
            Ok((77, "я буду через час".into()))
        );
        assert_eq!(to("Машуле привет"), Ok((77, "привет".into())));
        assert_eq!(to("маме: купи хлеб"), Ok((88, "купи хлеб".into())));
        assert_eq!(to("Сергею что ужин готов"), Ok((42, "ужин готов".into())));
        assert_eq!(to("Диме, позвони"), Ok((99, "позвони".into())));
        assert_eq!(to("Ёжику привет"), Err(Miss::Unknown("ежику".into())));
        assert_eq!(to("Маше что"), Err(Miss::Empty));
    }

    #[test]
    fn same_looking_names_are_not_guessed() {
        let p: Vec<Person> = [(1, "Саша"), (2, "Сашка")]
            .into_iter()
            .map(|(id, name)| Person {
                id,
                name: name.into(),
            })
            .collect();
        assert_eq!(address("Саше привет", &p).unwrap_err(), Miss::Ambiguous);
        // The same person twice (an owner who is also a listener) is no ambiguity.
        let p: Vec<Person> = [(1, "Саша"), (1, "Саша")]
            .into_iter()
            .map(|(id, name)| Person {
                id,
                name: name.into(),
            })
            .collect();
        assert_eq!(address("Саше привет", &p).unwrap().1, "привет");
        // Too little in common is not a name.
        assert_eq!(
            address(
                "Сан привет",
                &[Person {
                    id: 1,
                    name: "Сабина".into()
                }]
            )
            .unwrap_err(),
            Miss::Unknown("сан".into())
        );
    }

    #[test]
    fn spoken_names_drop_emoji_and_marks() {
        assert_eq!(spoken_name("Маша🌸"), "Маша");
        assert_eq!(spoken_name("Дима К."), "Дима К");
    }
}
