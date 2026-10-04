import 'package:flutter/material.dart';

import '../../copy.dart';

class SignInScreen extends StatelessWidget {
  const SignInScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              Copy.productName,
              style: TextStyle(fontSize: 28, fontWeight: FontWeight.bold),
            ),
            SizedBox(height: 12),
            Padding(
              padding: EdgeInsets.symmetric(horizontal: 24),
              child: Text(Copy.productDescription, textAlign: TextAlign.center),
            ),
            SizedBox(height: 24),
            ElevatedButton(
              onPressed: null,
              child: Text(Copy.continueWithGoogle),
            ),
          ],
        ),
      ),
    );
  }
}
