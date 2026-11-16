use std::fmt;
use std::str::FromStr;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct IntervalArg {
    duration: Duration,
}

impl FromStr for IntervalArg {
    type Err = String;

    fn from_str(s: &str) -> Result<IntervalArg, String> {
        let secs = match s.parse::<f64>() {
            Ok(secs) => secs,
            Err(err) => return Err(format!("{err}")),
        };
        if !secs.is_finite() || secs < 0.0 {
            return Err(format!("invalid interval: {s}"));
        }
        Ok(IntervalArg {
            duration: Duration::from_secs_f64(secs),
        })
    }
}

impl IntervalArg {
    pub(crate) fn as_duration(&self) -> Duration {
        self.duration
    }
}

impl fmt::Display for IntervalArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.duration.as_secs_f64())
    }
}
