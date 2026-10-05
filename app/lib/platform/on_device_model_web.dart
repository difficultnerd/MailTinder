import 'dart:js_interop';
import 'dart:js_interop_unsafe';

import 'on_device_model.dart';

/// Web [OnDeviceModel] over the built-in Prompt API (the `LanguageModel`
/// global in current Chrome) [ASSUMES]. It never triggers a model download.
class OnDeviceModelImpl implements OnDeviceModel {
  const OnDeviceModelImpl();

  JSObject? get _global {
    final value = globalContext.getProperty<JSAny?>('LanguageModel'.toJS);
    return value.isA<JSObject>() ? value as JSObject : null;
  }

  @override
  Future<bool> isAvailable() async {
    try {
      final global = _global;
      if (global == null) return false;
      final state = await global
          .callMethod<JSPromise<JSAny?>>('availability'.toJS)
          .toDart;
      return state.isA<JSString>() && (state as JSString).toDart == 'available';
    } on Object {
      return false;
    }
  }

  @override
  Future<String?> prompt(
    String text, {
    Duration timeout = const Duration(milliseconds: 800),
  }) async {
    try {
      final global = _global;
      if (global == null) return null;
      final reply = await Future<String?>(() async {
        final session = await global
            .callMethod<JSPromise<JSObject>>('create'.toJS)
            .toDart;
        try {
          final out = await session
              .callMethod<JSPromise<JSAny?>>('prompt'.toJS, text.toJS)
              .toDart;
          return out.isA<JSString>() ? (out as JSString).toDart : null;
        } finally {
          session.callMethod<JSAny?>('destroy'.toJS);
        }
      }).timeout(timeout);
      return reply;
    } on Object {
      return null;
    }
  }
}
