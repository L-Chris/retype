import 'dart:async';
import 'dart:io';

import 'package:crypto/crypto.dart';

/// Fixed upstream revision and hashes keep an upstream edit from changing a
/// user's dictionary silently. Optional data is never part of the installer.
const dictionaryRevision = 'v18.0.14';

class DictionaryPack {
  const DictionaryPack(
    this.id,
    this.title,
    this.description,
    this.bytes,
    this.sha256,
  );
  final String id;
  final String title;
  final String description;
  final int bytes;
  final String sha256;

  Uri get source => Uri.https(
    'raw.githubusercontent.com',
    '/amzxyz/rime-wanxiang/$dictionaryRevision/dicts/$id.dict.yaml',
  );
}

const dictionaryPacks = <DictionaryPack>[
  DictionaryPack(
    'mingren',
    '名人',
    '作家、历史人物等',
    551174,
    '80f169636d768114e97cf11ccf652333ada4db2e235f47aeba7baf808256338f',
  ),
  DictionaryPack(
    'renming',
    '常用人名',
    '常见中文姓名',
    558210,
    'bfd5439b6b1db4453ecba7bccf33948ecfdff33cfc931f1b97db0fb889400b2b',
  ),
  DictionaryPack(
    'yiren',
    '艺人',
    '演员、歌手等',
    361727,
    '0ae50f62844eb04f736fec9442707beb5938220b959db9ecfdca0890db809f39',
  ),
  DictionaryPack(
    'diming',
    '地名',
    '国内外地理名称',
    3147736,
    'e5519b85e37159fdf5f71ca342d4b0dfcc9e89847012b2e38f6d8904e8cd3193',
  ),
  DictionaryPack(
    'yixue',
    '医学',
    '医学名词和术语',
    826294,
    'a38c29aef6130aff840ab94bd83641ab6061226ca0ebfef007b0ab8fd5f5eaf8',
  ),
  DictionaryPack(
    'yaopin',
    '药品',
    '药品与成分名称',
    574422,
    '9596228b5a9ef5cc9202653d3b7da599900182a064bc9b8538781d9bd9c45c4d',
  ),
  DictionaryPack(
    'huaxue',
    '化学',
    '化学名词和物质',
    793045,
    'd886b18c68541a94fb0c189bb0c5e00f77fef99f9d2e14c80bf3d6cd477e9070',
  ),
];

class DictionaryPackState {
  const DictionaryPackState({
    required this.pack,
    required this.installed,
    required this.enabled,
  });
  final DictionaryPack pack;
  final bool installed;
  final bool enabled;
}

class PackDownloadControl {
  HttpClient? _client;
  bool cancelled = false;

  void cancel() {
    cancelled = true;
    _client?.close(force: true);
  }
}

class DictionaryStore {
  DictionaryStore(
    this.root,
    this.builder, {
    Uri Function(DictionaryPack)? source,
  }) : source = source ?? ((pack) => pack.source);

  final Directory root;
  final String builder;
  final Uri Function(DictionaryPack) source;

  File _binary(DictionaryPack pack) => File('${root.path}\\${pack.id}.bin');
  File _checksum(DictionaryPack pack) =>
      File('${root.path}\\${pack.id}.sha256');

  Future<bool> isInstalled(DictionaryPack pack) async {
    final binary = _binary(pack);
    final checksum = _checksum(pack);
    if (!await binary.exists() || !await checksum.exists()) return false;
    try {
      final expected = (await checksum.readAsString()).trim();
      if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(expected)) return false;
      return (await sha256.bind(binary.openRead()).first).toString() ==
          expected;
    } on FileSystemException {
      return false;
    } on FormatException {
      return false;
    }
  }

  Future<void> install(
    DictionaryPack pack, {
    void Function(double)? progress,
    PackDownloadControl? control,
  }) async {
    await root.create(recursive: true);
    final nonce = DateTime.now().microsecondsSinceEpoch;
    final raw = File('${root.path}\\${pack.id}-$nonce.yaml');
    final tsv = File('${root.path}\\${pack.id}-$nonce.tsv');
    final stagedBinary = File('${root.path}\\${pack.id}-$nonce.bin');
    final client = HttpClient()
      ..connectionTimeout = const Duration(seconds: 20);
    control?._client = client;
    try {
      if (control?.cancelled == true) throw StateError('下载已取消');
      final request = await client
          .getUrl(source(pack))
          .timeout(const Duration(seconds: 30));
      final response = await request.close().timeout(
        const Duration(seconds: 30),
      );
      if (response.statusCode != HttpStatus.ok) {
        throw StateError('下载失败：HTTP ${response.statusCode}');
      }
      final sink = raw.openWrite();
      var received = 0;
      try {
        await for (final chunk in response.timeout(
          const Duration(seconds: 30),
        )) {
          if (control?.cancelled == true) throw StateError('下载已取消');
          received += chunk.length;
          if (received > 8 * 1024 * 1024) throw StateError('下载的词库超过大小限制');
          sink.add(chunk);
          progress?.call((received / pack.bytes).clamp(0.0, 1.0));
        }
        await sink.flush();
      } finally {
        await sink.close();
      }
      if (received != pack.bytes ||
          (await sha256.bind(raw.openRead()).first).toString() != pack.sha256) {
        throw StateError('词库校验失败，请重试');
      }
      if (control?.cancelled == true) throw StateError('下载已取消');
      final result = await Process.run(builder, [
        '--in',
        raw.path,
        '--out',
        tsv.path,
        '--max-word-len',
        '8',
        '--no-verify',
      ]);
      if (result.exitCode != 0 || !await stagedBinary.exists()) {
        throw StateError('词库转换失败：${(result.stderr as String).trim()}');
      }
      if (control?.cancelled == true) throw StateError('下载已取消');
      final digest = (await sha256.bind(stagedBinary.openRead()).first)
          .toString();
      if (await _binary(pack).exists()) await _binary(pack).delete();
      await stagedBinary.rename(_binary(pack).path);
      await _checksum(pack).writeAsString(digest);
    } finally {
      control?._client = null;
      client.close(force: true);
      for (final file in [raw, tsv, stagedBinary]) {
        if (await file.exists()) await file.delete();
      }
    }
  }

  Future<void> delete(DictionaryPack pack) async {
    for (final file in [_binary(pack), _checksum(pack)]) {
      if (await file.exists()) await file.delete();
    }
  }
}
