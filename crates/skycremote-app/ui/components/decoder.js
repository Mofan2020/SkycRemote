/*!
 * SkycRemote UI — H.264 decoder
 *
 * ---------------------------------------------------------------------
 * The backend sends H.264 in Annex-B: the pieces are separated by
 * start codes (00 00 01).  That is what the encoder produces, and it
 * is what every video tool speaks.
 *
 * The browser's decoder speaks AVCC instead: the same pieces, but each
 * one carries its length in front, and the two parameter sets (the
 * picture size and so on) are written down separately instead of being
 * mixed in with the pictures.
 *
 * So the first job here is turning Annex-B into AVCC.  The second is
 * handing the result to WebCodecs and putting the pictures on the
 * canvas.
 *
 * A decoder needs a key picture to start from.  Pictures that arrive
 * before one are dropped, because there is nothing to decode them
 * against.
 * ---------------------------------------------------------------------
 */
(function (global) {
  'use strict';

  var NS = (global.SkycUI = global.SkycUI || {});

  /* ------------------------------------------------------------------
   * Annex-B
   * ---------------------------------------------------------------- */

  /**
   * Split Annex-B bytes into their pieces.
   * @param {Uint8Array} bytes
   * @returns {Array<Uint8Array>}
   */
  function splitPieces(bytes) {
    var starts = [];
    var i = 0;
    while (i + 2 < bytes.length) {
      if (bytes[i] === 0 && bytes[i + 1] === 0) {
        if (bytes[i + 2] === 1) {
          starts.push({ at: i, len: 3 });
          i += 3;
          continue;
        }
        // Four byte start code: 00 00 00 01
        if (bytes[i + 2] === 0 && i + 3 < bytes.length && bytes[i + 3] === 1) {
          starts.push({ at: i, len: 4 });
          i += 4;
          continue;
        }
      }
      i += 1;
    }

    var pieces = [];
    for (var k = 0; k < starts.length; k++) {
      var from = starts[k].at + starts[k].len;
      var to = k + 1 < starts.length ? starts[k + 1].at : bytes.length;
      if (to > from) pieces.push(bytes.subarray(from, to));
    }
    return pieces;
  }

  /** The kind of a piece: 7 is the picture size, 8 is the other one. */
  function pieceKind(piece) {
    return piece[0] & 0x1f;
  }

  /** Whether a piece is a picture that can stand on its own. */
  function isKeyPicture(pieces) {
    for (var i = 0; i < pieces.length; i++) {
      // 5 is the picture that resets everything before it.
      if (pieceKind(pieces[i]) === 5) return true;
    }
    return false;
  }

  /* ------------------------------------------------------------------
   * AVCC
   * ---------------------------------------------------------------- */

  /**
   * Put the pieces together the way the browser wants them: each one
   * with its length in front.
   * @param {Array<Uint8Array>} pieces
   * @returns {Uint8Array}
   */
  function toAvcc(pieces) {
    var total = 0;
    for (var i = 0; i < pieces.length; i++) total += 4 + pieces[i].length;

    var out = new Uint8Array(total);
    var at = 0;
    for (var j = 0; j < pieces.length; j++) {
      var length = pieces[j].length;
      out[at] = (length >>> 24) & 0xff;
      out[at + 1] = (length >>> 16) & 0xff;
      out[at + 2] = (length >>> 8) & 0xff;
      out[at + 3] = length & 0xff;
      out.set(pieces[j], at + 4);
      at += 4 + length;
    }
    return out;
  }

  /**
   * The description the browser needs before it can decode: the two
   * parameter sets, and what they say about the stream.
   * @param {Uint8Array} sps
   * @param {Uint8Array} pps
   * @returns {Uint8Array}
   */
  function description(sps, pps) {
    var out = new Uint8Array(11 + sps.length + pps.length);
    var at = 0;
    out[at++] = 1;              // the version of this description
    out[at++] = sps[1];         // what the pictures look like
    out[at++] = sps[2];         // what they are compatible with
    out[at++] = sps[3];         // how hard they are
    out[at++] = 0xff;           // the lengths in front are four bytes
    out[at++] = 0xe1;           // and there is one picture size
    out[at++] = (sps.length >> 8) & 0xff;
    out[at++] = sps.length & 0xff;
    out.set(sps, at);
    at += sps.length;
    out[at++] = 1;              // and one of the other
    out[at++] = (pps.length >> 8) & 0xff;
    out[at++] = pps.length & 0xff;
    out.set(pps, at);
    return out;
  }

  /**
   * The name of the codec, taken from the picture size so that it
   * always matches what is actually being decoded.
   * @param {Uint8Array} sps
   * @returns {string}
   */
  function codecName(sps) {
    function hex(value) {
      var text = value.toString(16);
      return text.length < 2 ? '0' + text : text;
    }
    return 'avc1.' + hex(sps[1]) + hex(sps[2]) + hex(sps[3]);
  }

  /* ------------------------------------------------------------------
   * Text to bytes
   * ---------------------------------------------------------------- */

  /**
   * Turn the text the backend sent into bytes.
   * @param {string} text
   * @returns {Uint8Array}
   */
  function fromText(text) {
    var binary = global.atob(text);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return bytes;
  }

  /* ------------------------------------------------------------------
   * The decoder
   * ---------------------------------------------------------------- */

  /**
   * Decodes the pictures and puts them on a canvas.
   *
   * @param {HTMLCanvasElement} canvas
   */
  function Decoder(canvas) {
    this.canvas = canvas;
    this.context = canvas ? canvas.getContext('2d') : null;
    this.decoder = null;
    this.codec = null;
    /** How many pictures have been drawn. */
    this.drawn = 0;
    /** How many pictures were dropped because there was no key picture. */
    this.dropped = 0;
  }

  /** Whether this browser can decode H.264 at all. */
  Decoder.supported = function () {
    return typeof global.VideoDecoder === 'function' &&
      typeof global.EncodedVideoChunk === 'function';
  };

  /**
   * Give it one picture.
   *
   * @param {string} text  The picture, as the text the backend sent it.
   * @param {number} timestampNs  When the picture should be shown.
   * @returns {boolean}  Whether it was handed to the browser.
   */
  Decoder.prototype.feed = function (text, timestampNs) {
    if (!Decoder.supported() || !this.context) return false;

    var pieces;
    try {
      pieces = splitPieces(fromText(text));
    } catch (error) {
      return false;
    }
    if (!pieces.length) return false;

    var sps = null;
    var pps = null;
    var pictures = [];
    for (var i = 0; i < pieces.length; i++) {
      var kind = pieceKind(pieces[i]);
      if (kind === 7) { sps = pieces[i]; continue; }
      if (kind === 8) { pps = pieces[i]; continue; }
      pictures.push(pieces[i]);
    }
    if (!sps || !pps) {
      // Without the two parameter sets there is nothing to decode
      // against.  Wait for a key picture that carries them.
      this.dropped += 1;
      return false;
    }

    var codec = codecName(sps);
    if (!this.decoder || this.codec !== codec) {
      this.reset();
      try {
        this.decoder = new global.VideoDecoder({
          output: this.onPicture.bind(this),
          error: function (error) {
            if (global.console && console.error) {
              console.error('[skycremote] the decoder had a problem', error);
            }
          }
        });
        this.decoder.configure({
          codec: codec,
          description: description(sps, pps),
          optimizeForLatency: true
        });
        this.codec = codec;
      } catch (error) {
        this.decoder = null;
        return false;
      }
    }

    if (!this.decoder || this.decoder.state !== 'configured') return false;

    try {
      this.decoder.decode(new global.EncodedVideoChunk({
        type: isKeyPicture(pieces) ? 'key' : 'delta',
        timestamp: Math.round((timestampNs || 0) / 1000),
        data: toAvcc(pictures)
      }));
      return true;
    } catch (error) {
      return false;
    }
  };

  /** Throw the decoder away, so that the next picture builds a new one. */
  Decoder.prototype.reset = function () {
    if (this.decoder) {
      try { this.decoder.close(); } catch (ignored) { }
    }
    this.decoder = null;
    this.codec = null;
  };

  /**
   * Put one picture on the canvas.
   * @param {VideoFrame} frame
   */
  Decoder.prototype.onPicture = function (frame) {
    try {
      var canvas = this.canvas;
      if (canvas && this.context) {
        if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
          canvas.width = frame.displayWidth;
          canvas.height = frame.displayHeight;
        }
        this.context.drawImage(frame, 0, 0);
        this.drawn += 1;
      }
    } finally {
      frame.close();
    }
  };

  NS.Decoder = Decoder;
  NS.h264 = {
    splitPieces: splitPieces,
    toAvcc: toAvcc,
    description: description,
    codecName: codecName,
    isKeyPicture: isKeyPicture,
    fromText: fromText
  };
})(typeof window !== 'undefined' ? window : this);
