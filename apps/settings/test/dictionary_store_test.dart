import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:retype/dictionary_store.dart';

void main() {
  final sourceDirectory = Platform.environment['RETYPE_PACK_TEST_SOURCE_DIR'];
  final builderPath = Platform.environment['RETYPE_PACK_TEST_BUILDER'];
  test('downloads, verifies and compiles all pinned packs', () async {
    final root = await Directory.systemTemp.createTemp(
      'retype-pack-integration-',
    );
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final id = request.uri.pathSegments.last;
      final sourceFile = File('$sourceDirectory\\$id.dict.yaml');
      await request.response.addStream(sourceFile.openRead());
      await request.response.close();
    });
    final store = DictionaryStore(
      root,
      builderPath!,
      source: (pack) => Uri.parse('http://127.0.0.1:${server.port}/${pack.id}'),
    );
    try {
      for (final pack in dictionaryPacks) {
        await store.install(pack);
        expect(await store.isInstalled(pack), true, reason: pack.id);
        expect(
          await File('${root.path}\\${pack.id}.bin').length(),
          greaterThan(1000),
        );
        await store.delete(pack);
        expect(await store.isInstalled(pack), false);
      }
    } finally {
      await server.close(force: true);
      await root.delete(recursive: true);
    }
  }, skip: sourceDirectory == null || builderPath == null);

  test('a corrupted download cannot become an installed pack', () async {
    final root = await Directory.systemTemp.createTemp('retype-pack-test-');
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      request.response.add([1, 2, 3]);
      await request.response.close();
    });
    const pack = DictionaryPack(
      'mingren',
      '名人',
      '',
      3,
      '0000000000000000000000000000000000000000000000000000000000000000',
    );
    final store = DictionaryStore(
      root,
      'missing-builder.exe',
      source: (_) => Uri.parse('http://127.0.0.1:${server.port}/pack'),
    );
    try {
      await expectLater(store.install(pack), throwsStateError);
      expect(await store.isInstalled(pack), false);
      expect(await root.list().toList(), isEmpty);
    } finally {
      await server.close(force: true);
      await root.delete(recursive: true);
    }
  });

  test('an installed binary must match its local checksum', () async {
    final root = await Directory.systemTemp.createTemp('retype-pack-test-');
    const pack = DictionaryPack('mingren', '名人', '', 3, '');
    final store = DictionaryStore(root, 'unused.exe');
    try {
      final data = [1, 2, 3];
      final binary = File('${root.path}\\mingren.bin');
      final checksum = File('${root.path}\\mingren.sha256');
      await binary.writeAsBytes(data);
      await checksum.writeAsString(sha256.convert(data).toString());
      expect(await store.isInstalled(pack), true);
      await binary.writeAsBytes([1, 2, 4]);
      expect(await store.isInstalled(pack), false);
    } finally {
      await root.delete(recursive: true);
    }
  });
}
