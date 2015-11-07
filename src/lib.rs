use std::num;
use std::str;
use std::fmt;
use std::io;
use std::io::Read;
use std::io::Write;

const MAX_PARAMS_SIZE: usize = 1024;
const FILE_MAGICK: &'static [u8] = b"YUV4MPEG2 ";
const FRAME_MAGICK: &'static [u8] = b"FRAME";
const TERMINATOR: u8 = 0x0A;
const SEPARATOR: u8 = b' ';

/// Both encoding and decoding errors.
#[derive(Debug)]
pub enum Error {
    /// End of the file. Technically not an error, but it's easier to process
    /// that way.
    EOF,
    /// Bad input parameters provided.
    BadInput,
    /// Error while parsing the file/frame header.
    // TODO(Kagami): Better granularity of parse errors.
    ParseError,
    /// Error while reading/writing the file.
    IoError(io::Error),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error { Error::IoError(err) }
}

impl From<num::ParseIntError> for Error {
    fn from(_: num::ParseIntError) -> Error { Error::ParseError }
}

impl From<str::Utf8Error> for Error {
    fn from(_: str::Utf8Error) -> Error { Error::ParseError }
}

trait EnhancedRead {
    fn read_until(&mut self, ch: u8, buf: &mut [u8]) -> Result<usize, Error>;
    // While Read::read_exact is unstable.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error>;
}

impl<R: Read> EnhancedRead for R {
    // Current implementation does one `read` call per byte. This might be a
    // bit slow for long headers but it simplifies things: we don't need to
    // check whether start of the next frame is already read and so on.
    fn read_until(&mut self, ch: u8, buf: &mut [u8]) -> Result<usize, Error> {
        let mut collected = 0;
        while collected < buf.len() {
            let chunk_size = try!(self.read(&mut buf[collected..collected+1]));
            if chunk_size == 0 {
                return Err(Error::EOF);
            }
            if buf[collected] == ch {
                return Ok(collected);
            }
            collected += chunk_size;
        }
        Err(Error::ParseError)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let mut collected = 0;
        while collected < buf.len() {
            let chunk_size = try!(self.read(&mut buf[collected..]));
            if chunk_size == 0 {
                return Err(Error::EOF);
            }
            collected += chunk_size;
        }
        Ok(())
    }
}

fn parse_bytes(buf: &[u8]) -> Result<usize, Error> {
    // A bit kludgy but seems like there is no other way.
    Ok(try!(try!(str::from_utf8(buf)).parse()))
}

/// Simple Ratio structure since it's not available in stdlib.
#[derive(Debug, Clone, Copy)]
pub struct Ratio {
    num: usize,
    den: usize,
}

impl Ratio {
    pub fn new(num: usize, den: usize) -> Ratio {
        Ratio {num: num, den: den}
    }
}

/// **NOTE:** Only 8-bit formats are currently supported.
///
/// > yuv4mpeg can only handle yuv444p, yuv422p, yuv420p, yuv411p and gray8
/// pixel formats. And using 'strict -1' also yuv444p9, yuv422p9, yuv420p9,
/// yuv444p10, yuv422p10, yuv420p10, yuv444p12, yuv422p12, yuv420p12,
/// yuv444p14, yuv422p14, yuv420p14, yuv444p16, yuv422p16, yuv420p16 and gray16
/// pixel formats.
///
/// (c) ffmpeg.
#[derive(Debug, Clone, Copy)]
pub enum Colorspace {
    Cmono,
    C420,
    C422,
    C444,
    C420jpeg,
    C420paldv,
    /// Found in some files.
    C420mpeg2,
}

fn get_plane_sizes(
    width: usize, height: usize, colorspace: Option<Colorspace>,
) -> (usize, usize, usize) {
    let pixels = width * height;
    let c420_sizes = (pixels, pixels/4, pixels/4);
    match colorspace {
        Some(Colorspace::Cmono) => (pixels, 0, 0),
        Some(Colorspace::C420) => c420_sizes,
        Some(Colorspace::C422) => (pixels, pixels/2, pixels/2),
        Some(Colorspace::C444) => (pixels, pixels, pixels),
        Some(Colorspace::C420jpeg) => c420_sizes,
        Some(Colorspace::C420paldv) => c420_sizes,
        Some(Colorspace::C420mpeg2) => c420_sizes,
        None => c420_sizes,
    }
}

pub struct Decoder<'d, R: Read + 'd> {
    reader: &'d mut R,
    params_buf: Vec<u8>,
    frame_buf: Vec<u8>,
    raw_params: Vec<u8>,
    width: usize,
    height: usize,
    framerate: Ratio,
    colorspace: Option<Colorspace>,
    y_len: usize,
    u_len: usize,
}

impl<'d, R: Read> fmt::Debug for Decoder<'d, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "y4m::Decoder<w={}, h={}>", self.width, self.height)
    }
}

impl<'d, R: Read> Decoder<'d, R> {
    pub fn new(reader: &mut R) -> Result<Decoder<R>, Error> {
        let mut params_buf = vec![0;MAX_PARAMS_SIZE];
        let end_params_pos = try!(reader.read_until(TERMINATOR, &mut params_buf));
        if end_params_pos < FILE_MAGICK.len() || !params_buf.starts_with(FILE_MAGICK) {
            return Err(Error::ParseError);
        }
        let raw_params = (&params_buf[FILE_MAGICK.len()..end_params_pos]).to_owned();
        let mut width = 0;
        let mut height = 0;
        let mut fps = Ratio::new(0, 0);
        let mut csp = None;
        // We shouldn't convert it to string because encoding is unspecified.
        for param in raw_params.split(|&b| b == SEPARATOR) {
            if param.len() < 1 { continue }
            let (name, value) = (param[0], &param[1..]);
            // TODO(Kagami): frame rate, interlacing, pixel aspect, comment.
            match name {
                b'W' => { width = try!(parse_bytes(value)) },
                b'H' => { height = try!(parse_bytes(value)) },
                b'F' => {

                },
                b'C' => {
                    csp = match value {
                        b"mono" => Some(Colorspace::Cmono),
                        b"420" => Some(Colorspace::C420),
                        b"422" => Some(Colorspace::C422),
                        b"444" => Some(Colorspace::C444),
                        b"420jpeg" => Some(Colorspace::C420jpeg),
                        b"420paldv" => Some(Colorspace::C420paldv),
                        b"420mpeg2" => Some(Colorspace::C420mpeg2),
                        _ => None,
                    };
                },
                _ => {},
            }
        }
        if width == 0 || height == 0 { return Err(Error::ParseError) }
        let (y_len, u_len, v_len) = get_plane_sizes(width, height, csp);
        let frame_size = y_len + u_len + v_len;
        let frame_buf = vec![0;frame_size];
        Ok(Decoder {
            reader: reader,
            params_buf: params_buf,
            frame_buf: frame_buf,
            raw_params: raw_params,
            width: width,
            height: height,
            framerate: fps,
            colorspace: csp,
            y_len: y_len,
            u_len: u_len,
        })
    }

    /// Iterate over frames, without extra heap allocations. End of input is
    /// indicated by `Error::EOF`.
    pub fn read_frame(&mut self) -> Result<Frame, Error> {
        let end_params_pos = try!(self.reader.read_until(TERMINATOR, &mut self.params_buf));
        if end_params_pos < FRAME_MAGICK.len() || !self.params_buf.starts_with(FRAME_MAGICK) {
            return Err(Error::ParseError);
        }
        // We don't parse frame params currently but user has access to them.
        let start_params_pos = FRAME_MAGICK.len();
        let raw_params = if end_params_pos - start_params_pos > 0 {
            // Check for extra space.
            if self.params_buf[start_params_pos] != SEPARATOR {
                return Err(Error::ParseError);
            }
            Some((&self.params_buf[start_params_pos+1..end_params_pos]).to_owned())
        } else {
            None
        };
        try!(self.reader.read_exact(&mut self.frame_buf));
        Ok(Frame::new([
            &self.frame_buf[0..self.y_len],
            &self.frame_buf[self.y_len..self.y_len+self.u_len],
            &self.frame_buf[self.y_len+self.u_len..],
        ], raw_params))
    }

    #[inline]
    pub fn get_width(&self) -> usize { self.width }
    #[inline]
    pub fn get_height(&self) -> usize { self.height }
    #[inline]
    pub fn get_framerate(&self) -> Ratio { self.framerate }
    /// Return file colorspace.
    ///
    /// **NOTE:** normally all .y4m should have colorspace param, but there are
    /// files encoded without that tag and it's unclear what should we do in
    /// that case. Currently C420 is implied by default as per ffmpeg behavior.
    #[inline]
    pub fn get_colorspace(&self) -> Option<Colorspace> { self.colorspace }
    #[inline]
    pub fn get_raw_params(&self) -> &[u8] { &self.raw_params }
}

#[derive(Debug)]
pub struct Frame<'f> {
    planes: [&'f [u8];3],
    raw_params: Option<Vec<u8>>,
}

impl<'f> Frame<'f> {
    /// Create a new frame with optional parameters.
    /// No heap allocations are made.
    pub fn new(planes: [&'f [u8];3], raw_params: Option<Vec<u8>>) -> Frame<'f> {
        Frame {planes: planes, raw_params: raw_params}
    }

    #[inline]
    pub fn get_y_plane(&self) -> &[u8] { self.planes[0] }
    #[inline]
    pub fn get_u_plane(&self) -> &[u8] { self.planes[1] }
    #[inline]
    pub fn get_v_plane(&self) -> &[u8] { self.planes[2] }
    #[inline]
    pub fn get_raw_params(&self) -> Option<&[u8]> { self.raw_params.as_ref().map(|v| &v[..]) }
}

/// Encoder builder. Allows to set y4m file parameters using builder pattern.
// TODO(Kagami): Accept all known tags and raw params.
#[derive(Debug)]
pub struct EncoderBuilder {
    width: usize,
    height: usize,
    colorspace: Option<Colorspace>,
}

impl EncoderBuilder {
    /// Create a new encoder builder.
    pub fn new(width: usize, height: usize) -> EncoderBuilder {
        EncoderBuilder {
            width: width,
            height: height,
            colorspace: None,
        }
    }

    /// Specify file colorspace.
    pub fn with_colorspace(mut self, colorspace: Colorspace) -> Self {
        self.colorspace = Some(colorspace);
        self
    }

    /// Write header to the stream and create encoder instance.
    pub fn write_header<W: Write>(self, writer: &mut W) -> Result<Encoder<W>, Error> {
        // XXX(Kagami): Beware that FILE_MAGICK already contains space.
        try!(writer.write_all(FILE_MAGICK));
        try!(write!(writer, "W{} H{}", self.width, self.height));
        match self.colorspace {
            Some(csp) => try!(write!(writer, " {:?}", csp)),
            _ => {},
        }
        try!(writer.write_all(&[TERMINATOR]));
        let (y_len, u_len, v_len) = get_plane_sizes(self.width, self.height, self.colorspace);
        Ok(Encoder {
            writer: writer,
            y_len: y_len,
            u_len: u_len,
            v_len: v_len,
        })
    }
}

pub struct Encoder<'e, W: Write + 'e> {
    writer: &'e mut W,
    y_len: usize,
    u_len: usize,
    v_len: usize,
}

impl<'e, W: Write> fmt::Debug for Encoder<'e, W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "y4m::Encoder")
    }
}

impl<'e, W: Write> Encoder<'e, W> {
    /// Write next frame to the stream.
    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), Error> {
        if frame.get_y_plane().len() != self.y_len
            ||  frame.get_u_plane().len() != self.u_len
            ||  frame.get_v_plane().len() != self.v_len {
            return Err(Error::BadInput);
        }
        try!(self.writer.write_all(FRAME_MAGICK));
        match frame.get_raw_params() {
            Some(params) => {
                try!(self.writer.write_all(&[SEPARATOR]));
                try!(self.writer.write_all(params));
            },
            _ => {},
        }
        try!(self.writer.write_all(&[TERMINATOR]));
        try!(self.writer.write_all(frame.get_y_plane()));
        try!(self.writer.write_all(frame.get_u_plane()));
        try!(self.writer.write_all(frame.get_v_plane()));
        Ok(())
    }
}

/// Create a new decoder instance. Alias for `Decoder::new`.
pub fn decode<R: Read>(reader: &mut R) -> Result<Decoder<R>, Error> {
    Decoder::new(reader)
}

/// Create a new encoder builder. Alias for `EncoderBuilder::new`.
pub fn encode(width: usize, height: usize) -> EncoderBuilder {
    EncoderBuilder::new(width, height)
}
