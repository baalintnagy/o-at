//! Adapters binding the controls to NIH-plug parameters.

use egui::{Color32, Rect, Ui};
use nih_plug::prelude::{BoolParam, Enum, EnumParam, Param, ParamSetter};

use crate::control::{Control, Switch};
use crate::widgets::segmented;

/// A NIH-plug parameter as a [`Control`], with host automation gestures.
pub struct ParamControl<'a, P: Param> {
    param: &'a P,
    setter: &'a ParamSetter<'a>,
}

impl<'a, P: Param> ParamControl<'a, P> {
    pub fn new(param: &'a P, setter: &'a ParamSetter<'a>) -> Self {
        Self { param, setter }
    }
}

impl<P: Param> Control for ParamControl<'_, P> {
    fn key(&self) -> &str {
        self.param.name()
    }

    fn normalized(&self) -> f32 {
        self.param.modulated_normalized_value()
    }

    fn default_normalized(&self) -> f32 {
        self.param.default_normalized_value()
    }

    fn format(&self, normalized: f32) -> String {
        self.param.normalized_value_to_string(normalized, true)
    }

    fn begin_edit(&self) {
        self.setter.begin_set_parameter(self.param);
    }

    fn set_normalized(&self, normalized: f32) {
        self.setter.set_parameter_normalized(self.param, normalized);
    }

    fn end_edit(&self) {
        self.setter.end_set_parameter(self.param);
    }
}

/// A NIH-plug boolean parameter as a [`Switch`].
pub struct ParamSwitch<'a> {
    param: &'a BoolParam,
    setter: &'a ParamSetter<'a>,
}

impl<'a> ParamSwitch<'a> {
    pub fn new(param: &'a BoolParam, setter: &'a ParamSetter<'a>) -> Self {
        Self { param, setter }
    }
}

impl Switch for ParamSwitch<'_> {
    fn key(&self) -> &str {
        self.param.name()
    }

    fn is_on(&self) -> bool {
        self.param.value()
    }

    fn set_on(&self, on: bool) {
        self.setter.begin_set_parameter(self.param);
        self.setter.set_parameter(self.param, on);
        self.setter.end_set_parameter(self.param);
    }
}

/// Segmented selector bound to an enum parameter.
pub fn enum_selector<E: Enum + PartialEq + 'static>(
    ui: &mut Ui,
    setter: &ParamSetter,
    param: &EnumParam<E>,
    rect: Rect,
    color: Color32,
) {
    let labels = E::variants();
    let current = param.value().to_index();
    if let Some(index) = segmented(ui, param.name(), rect, labels, current, color) {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, E::from_index(index));
        setter.end_set_parameter(param);
    }
}
