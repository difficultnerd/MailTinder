import 'package:flutter/material.dart';

import '../../copy.dart';

class RequestInviteScreen extends StatelessWidget {
  const RequestInviteScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              Copy.requestAnInvite,
              style: TextStyle(fontSize: 24, fontWeight: FontWeight.bold),
            ),
            SizedBox(height: 16),
            ElevatedButton(onPressed: null, child: Text(Copy.requestAnInvite)),
          ],
        ),
      ),
    );
  }
}
