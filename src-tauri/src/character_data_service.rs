use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use nte_dps_tool::core::character_data::{
    CharacterDataError, CharacterDataProjection, load_character_data,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CharacterDataServiceError {
    Busy,
    Domain(CharacterDataError),
}

pub(crate) struct CharacterDataService {
    path: PathBuf,
    busy: AtomicBool,
}

struct CharacterDataPermit<'a>(&'a AtomicBool);

impl Drop for CharacterDataPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl CharacterDataService {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            busy: AtomicBool::new(false),
        }
    }

    pub(crate) fn snapshot(&self) -> Result<CharacterDataProjection, CharacterDataServiceError> {
        let _permit = self.reserve()?;
        load_character_data(&self.path).map_err(CharacterDataServiceError::Domain)
    }

    fn reserve(&self) -> Result<CharacterDataPermit<'_>, CharacterDataServiceError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CharacterDataServiceError::Busy)?;
        Ok(CharacterDataPermit(&self.busy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_operation_fails_fast_while_busy() {
        let service = CharacterDataService::new(PathBuf::from("private-character-path.json"));
        let permit = service.reserve().expect("reserve character data slot");
        assert!(service.busy.load(Ordering::Acquire));
        assert_eq!(service.snapshot(), Err(CharacterDataServiceError::Busy));
        drop(permit);
        assert!(!service.busy.load(Ordering::Acquire));
    }
}
