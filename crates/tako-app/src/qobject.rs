//! Minimal cxx-qt bridge for the Phase 0 §1 spike.
//!
//! `Greeting` is a QObject exposed to QML. Its `message` property flows
//! Rust → QML (Label binding); the `greet` invokable flows QML → Rust (Button
//! click) and then updates the property back. This proves both directions of
//! the cxx-qt bridge.

#![allow(unsafe_code)]

use core::pin::Pin;
use cxx_qt_lib::QString;

/// The bridge definition for our QObject.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, message)]
        #[namespace = "tako"]
        type Greeting = super::GreetingRust;

        #[qinvokable]
        #[cxx_name = "greet"]
        fn greet(self: Pin<&mut Self>, name: &QString);
    }
}

/// Rust backing struct for the `Greeting` QObject.
#[derive(Default)]
pub struct GreetingRust {
    message: QString,
}

impl qobject::Greeting {
    /// Build a greeting for `name` and publish it on the `message` property.
    pub fn greet(self: Pin<&mut Self>, name: &QString) {
        let text = format!("Hello, {name}! — from Rust");
        self.set_message(QString::from(text.as_str()));
    }
}
