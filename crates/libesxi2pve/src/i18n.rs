//! Türkçe/İngilizce dil seçimi. Ortam değişkenleri yalnızca okunur;
//! kütüphane tüketicileri her iş parçacığında ayrı dil kullanabilir.
use std::{cell::Cell, marker::PhantomData, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
	Turkish,
	English,
}
impl Language {
	pub fn code(self) -> &'static str {
		match self {
			Self::Turkish => "tr",
			Self::English => "en",
		}
	}
	pub fn from_locale(value: &str) -> Self {
		let normalized = value.trim().to_ascii_lowercase();
		let base = normalized.split(['_', '-', '.', '@']).next().unwrap_or("");
		if base == "tr" {
			Self::Turkish
		} else {
			Self::English
		}
	}
	/// ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG. Boş değerler atlanır.
	/// Tanınmayan dil ve C/POSIX için İngilizce kullanılır.
	pub fn from_environment() -> Self {
		Self::from_lookup(|name| std::env::var(name).ok())
	}
	pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
		for name in ["ESXI2PVE_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
			if let Some(value) = lookup(name).filter(|v| !v.trim().is_empty()) {
				return Self::from_locale(&value);
			}
		}
		Self::English
	}
}
thread_local! {
	static LANGUAGE: Cell<Option<Language>> = const { Cell::new(None) };
}
pub fn current_language() -> Language {
	LANGUAGE
		.with(|slot| slot.get())
		.unwrap_or_else(Language::from_environment)
}
/// Bu kapsamdan çıkınca önceki dil geri yüklenir. Başka iş parçacığına taşınamaz.
#[must_use]
pub struct LanguageScope {
	previous: Option<Language>,
	_not_send: PhantomData<Rc<()>>,
}
impl Drop for LanguageScope {
	fn drop(&mut self) {
		LANGUAGE.with(|slot| slot.set(self.previous));
	}
}
pub fn enter_language(language: Language) -> LanguageScope {
	let previous = LANGUAGE.with(|slot| slot.replace(Some(language)));
	LanguageScope {
		previous,
		_not_send: PhantomData,
	}
}
pub fn with_language<T>(language: Language, action: impl FnOnce() -> T) -> T {
	let _scope = enter_language(language);
	action()
}

// Dollar-token parametresi, iç makronun argümanlarını kararlı Rust'ta tanımlar.
// Çeviri şablonlarının ikisi de format! tarafından derleme anında denetlenir.
macro_rules! catalog {
	($d:tt; $( $turkish:tt => $english:literal, )*) => {
		#[macro_export]
		macro_rules! tr {
			$(
				($turkish $d(, $d($d args:tt)*)?) => {{
					match $crate::i18n::current_language() {
						$crate::i18n::Language::Turkish => format!($turkish $d(, $d($d args)*)?),
						$crate::i18n::Language::English => format!($english $d(, $d($d args)*)?),
					}
				}};
			)*
		}
	};
}
include!("messages.rs");

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn locale_variants_and_fallback() {
		for value in ["tr", "tr_TR.UTF-8", "TR-tr", "tr_TR@custom"] {
			assert_eq!(Language::from_locale(value), Language::Turkish);
		}
		for value in [
			"en",
			"en_US.UTF-8",
			"C",
			"C.UTF-8",
			"POSIX",
			"de_DE.UTF-8",
			"",
		] {
			assert_eq!(Language::from_locale(value), Language::English);
		}
	}
	#[test]
	fn environment_precedence_without_mutating_process_environment() {
		let pairs = [
			("ESXI2PVE_LANG", "en"),
			("LC_ALL", "tr_TR.UTF-8"),
			("LC_MESSAGES", "tr"),
			("LANG", "tr"),
		];
		let lookup = |name: &str| {
			pairs
				.iter()
				.find(|(key, _)| *key == name)
				.map(|(_, value)| value.to_string())
		};
		assert_eq!(Language::from_lookup(lookup), Language::English);
		assert_eq!(
			Language::from_lookup(|name| if name == "LC_ALL" {
				Some("tr_TR.UTF-8".into())
			} else {
				Some("".into())
			}),
			Language::Turkish
		);
		assert_eq!(
			Language::from_lookup(|name| if name == "LC_MESSAGES" {
				Some("tr".into())
			} else {
				None
			}),
			Language::Turkish
		);
		assert_eq!(
			Language::from_lookup(|name| if name == "LANG" {
				Some("tr".into())
			} else {
				None
			}),
			Language::Turkish
		);
		assert_eq!(Language::from_lookup(|_| None), Language::English);
	}
	#[test]
	fn nested_language_scopes_restore_previous_value() {
		with_language(Language::Turkish, || {
			assert_eq!(current_language(), Language::Turkish);
			with_language(Language::English, || {
				assert_eq!(current_language(), Language::English)
			});
			assert_eq!(current_language(), Language::Turkish);
		});
	}
	#[test]
	fn concurrent_language_scopes_do_not_mix() {
		let threads = [Language::Turkish, Language::English].map(|lang| {
			std::thread::spawn(move || {
				with_language(lang, || {
					for _ in 0..100 {
						assert_eq!(current_language(), lang);
					}
				})
			})
		});
		for thread in threads {
			thread.join().unwrap();
		}
	}
	#[test]
	fn formatted_catalog_message() {
		let key = "example";
		assert_eq!(
			with_language(Language::English, || tr!(
				"JSON alanı eksik: {key}",
				key = key
			)),
			"Missing JSON field: example"
		);
	}
}

#[cfg(test)]
mod restoration_tests {
	use super::*;
	#[test]
	fn language_is_restored_when_an_operation_panics() {
		with_language(Language::Turkish, || {
			let result = std::panic::catch_unwind(|| {
				with_language(Language::English, || panic!("test panic"))
			});
			assert!(result.is_err());
			assert_eq!(current_language(), Language::Turkish);
		});
	}
}
