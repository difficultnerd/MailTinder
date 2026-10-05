import 'on_device_model.dart';

/// Non-web [OnDeviceModel]: always unavailable.
class OnDeviceModelImpl implements OnDeviceModel {
  const OnDeviceModelImpl();

  @override
  Future<bool> isAvailable() async => false;

  @override
  Future<String?> prompt(
    String text, {
    Duration timeout = const Duration(milliseconds: 800),
  }) async => null;
}
