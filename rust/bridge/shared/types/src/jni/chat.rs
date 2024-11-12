//
// Copyright 2024 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

use super::*;
use crate::net::chat::{ChatListener, MakeChatListener, ServerMessageAck};

pub type JavaMakeChatListener<'a> = JObject<'a>;

pub struct JniChatListener {
    vm: JavaVM,
    listener: GlobalRef,
}

pub type JniMakeChatListener<'unused> = JniChatListener;

impl Clone for JniChatListener {
    fn clone(&self) -> Self {
        Self {
            // The next release of the JNI crate should make JavaVM implement Clone.
            // https://github.com/jni-rs/jni-rs/issues/503
            vm: unsafe {
                JavaVM::from_raw(self.vm.get_java_vm_pointer())
                    .expect("copied from existing pointer")
            },
            listener: self.listener.clone(),
        }
    }
}

impl JniChatListener {
    pub fn new(env: &mut JNIEnv<'_>, listener: &JObject) -> Result<Self, BridgeLayerError> {
        check_jobject_type(
            env,
            listener,
            ClassName("org.signal.libsignal.net.internal.MakeChatListener"),
        )?;
        Ok(Self {
            vm: env.get_java_vm().expect("can get VM"),
            listener: env.new_global_ref(listener).expect("can get env"),
        })
    }

    fn attach_and_log_on_error(
        &self,
        name: &'static str,
        operation: impl FnOnce(&mut JNIEnv<'_>) -> Result<(), BridgeLayerError>,
    ) {
        let attach_and_run = move || {
            let mut env = self.vm.attach_current_thread().expect("can attach thread");
            operation(&mut env)
        };
        match attach_and_run() {
            Ok(()) => {}
            Err(e) => {
                log::error!("failed to report {name}: {e}")
            }
        }
    }
}

impl MakeChatListener for JniChatListener {
    fn make_listener(&self) -> Box<dyn ChatListener> {
        Box::new(self.clone())
    }
}

impl ChatListener for JniChatListener {
    fn received_incoming_message(
        &mut self,
        envelope: Vec<u8>,
        timestamp: Timestamp,
        ack: ServerMessageAck,
    ) {
        let listener = &self.listener;
        self.attach_and_log_on_error("incoming message", move |env| {
            let env_array = envelope.convert_into(env)?;
            let ack_handle = ack.convert_into(env)?;
            call_method_checked(
                env,
                listener,
                "onIncomingMessage",
                jni_args!((
                    env_array => [byte],
                    timestamp.epoch_millis() as i64 => long,
                    ack_handle => long,
                ) -> void),
            )
        });
    }

    fn received_queue_empty(&mut self) {
        let listener = &self.listener;
        self.attach_and_log_on_error("queue empty", move |env| {
            call_method_checked(env, listener, "onQueueEmpty", jni_args!(() -> void))
        });
    }

    fn connection_interrupted(&mut self, disconnect_cause: ChatServiceError) {
        let listener = &self.listener;
        self.attach_and_log_on_error("connection interrupted", move |env| {
            convert_to_exception(
                env,
                SignalJniError::from(disconnect_cause),
                move |env, throwable, _error| {
                    throwable
                        .and_then(move |throwable| {
                            call_method_checked(
                                env,
                                listener,
                                "onConnectionInterrupted",
                                jni_args!((throwable => java.lang.Throwable) -> void),
                            )?;
                            Ok(())
                        })
                        .unwrap_or_else(|error| {
                            log::error!(
                                "failed to call onConnectionInterrupted with cause: {error}"
                            );
                        });
                },
            );
            Ok(())
        });
    }
}