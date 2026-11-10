use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrameSource {
    #[default]
    Mesa,
    MangoHud,
}

impl FrameSource {
    pub(crate) fn target(self) -> &'static str {
        match self {
            Self::Mesa => "Mesa Gallium renderer per-frame presentation interval",
            Self::MangoHud => "OpenGL GLX presentation-call interval",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Mesa => {
                "Mesa26.2.6 Gallium frametime per-present query, zero-period line-buffered dump"
            }
            Self::MangoHud => {
                "MangoHud0.8.4 OpenGL GLX hook, log_interval=0, newline-complete CSV frametime milliseconds and elapsed nanoseconds"
            }
        }
    }
}
