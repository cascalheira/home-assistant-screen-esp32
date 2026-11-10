//! Phantom-touch filter for the touch controller.

/// What the controller reported since the last poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    /// No new sample yet; keep the previous state.
    None,
    Pressed {
        x: i16,
        y: i16,
    },
    Released,
}

/// How far apart (in pixels, per axis) the two samples confirming a press
/// may be.
const CONFIRM_DISTANCE: i16 = 40;

/// Reports a press only after two consecutive samples close to each other
/// (about 10 ms apart): a real finger always gives several, while
/// electrical noise typically shows up as a single sample. A release is
/// reported only for presses that were reported.
#[derive(Debug, Default)]
pub struct TouchFilter {
    /// First sample of a press that isn't confirmed yet.
    candidate: Option<(i16, i16)>,
    /// A press has been reported and not released yet.
    down: bool,
}

impl TouchFilter {
    /// Takes the controller's raw sample, returns what the UI should see.
    pub fn feed(&mut self, raw: Sample) -> Sample {
        match raw {
            Sample::None => Sample::None,
            Sample::Pressed { x, y } => {
                if self.down {
                    return Sample::Pressed { x, y };
                }
                match self.candidate {
                    Some((cx, cy))
                        if (cx - x).abs() <= CONFIRM_DISTANCE
                            && (cy - y).abs() <= CONFIRM_DISTANCE =>
                    {
                        self.candidate = None;
                        self.down = true;
                        Sample::Pressed { x, y }
                    }
                    _ => {
                        self.candidate = Some((x, y));
                        Sample::None
                    }
                }
            }
            Sample::Released => {
                if self.candidate.take().is_some() && !self.down {
                    log::info!("ignored a one-sample touch (likely noise)");
                }
                if std::mem::take(&mut self.down) {
                    Sample::Released
                } else {
                    Sample::None
                }
            }
        }
    }
}
