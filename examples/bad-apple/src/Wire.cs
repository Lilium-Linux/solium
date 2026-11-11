// The protocol's encoding of values, which the generated types are read
// and written with: docs/window-managers.md, "The wire format".

using System;
using System.Buffers.Binary;
using System.IO;
using System.Text;

namespace Solium.Wm;

/// <summary>Writes values as the protocol encodes them.</summary>
public sealed class Writer
{
    private readonly MemoryStream _out = new();

    public byte[] ToArray() => _out.ToArray();

    public void Bool(bool value) => _out.WriteByte(value ? (byte)1 : (byte)0);

    // Unsigned integers are LEB128: seven bits a byte, low bits first, the
    // top bit set on every byte but the last.
    public void U8(byte value) => Leb(value);
    public void U16(ushort value) => Leb(value);
    public void U32(uint value) => Leb(value);
    public void U64(ulong value) => Leb(value);

    // Signed ones are zigzagged first, so small negative numbers stay small.
    public void S8(sbyte value) => S64(value);
    public void S16(short value) => S64(value);
    public void S32(int value) => S64(value);
    public void S64(long value) => Leb((ulong)((value << 1) ^ (value >> 63)));

    public void F32(float value)
    {
        Span<byte> bytes = stackalloc byte[4];
        BinaryPrimitives.WriteSingleLittleEndian(bytes, value);
        _out.Write(bytes);
    }

    public void F64(double value)
    {
        Span<byte> bytes = stackalloc byte[8];
        BinaryPrimitives.WriteDoubleLittleEndian(bytes, value);
        _out.Write(bytes);
    }

    public void Text(string value)
    {
        var bytes = Encoding.UTF8.GetBytes(value);
        Leb((ulong)bytes.Length);
        _out.Write(bytes);
    }

    private void Leb(ulong value)
    {
        while (true)
        {
            var low = (byte)(value & 0x7f);
            value >>= 7;
            if (value == 0)
            {
                _out.WriteByte(low);
                return;
            }
            _out.WriteByte((byte)(low | 0x80));
        }
    }
}

/// <summary>Reads values as the protocol encodes them.</summary>
public sealed class Reader
{
    private readonly byte[] _bytes;
    private int _at;

    public Reader(byte[] bytes) => _bytes = bytes;

    public bool Bool() => Byte() switch
    {
        0 => false,
        1 => true,
        var other => throw new InvalidDataException($"{other} is not a flag (0 or 1)"),
    };

    public byte U8() => checked((byte)Leb());
    public ushort U16() => checked((ushort)Leb());
    public uint U32() => checked((uint)Leb());
    public ulong U64() => Leb();
    public sbyte S8() => checked((sbyte)S64());
    public short S16() => checked((short)S64());
    public int S32() => checked((int)S64());

    public long S64()
    {
        var value = Leb();
        return (long)(value >> 1) ^ -(long)(value & 1);
    }

    public float F32() => BinaryPrimitives.ReadSingleLittleEndian(Take(4));
    public double F64() => BinaryPrimitives.ReadDoubleLittleEndian(Take(8));
    public string Text() => Encoding.UTF8.GetString(Take(Length()));

    /// <summary>A list's or a text's length: never more than is left to
    /// read, so a broken length cannot ask for a huge allocation.</summary>
    public int Length()
    {
        var length = Leb();
        if (length > (ulong)(_bytes.Length - _at))
        {
            throw new InvalidDataException("the message ended inside a value");
        }
        return (int)length;
    }

    private ReadOnlySpan<byte> Take(int count)
    {
        if (count > _bytes.Length - _at)
        {
            throw new InvalidDataException("the message ended inside a value");
        }
        var taken = _bytes.AsSpan(_at, count);
        _at += count;
        return taken;
    }

    private byte Byte() => Take(1)[0];

    private ulong Leb()
    {
        ulong value = 0;
        var shift = 0;
        while (true)
        {
            var next = Byte();
            if (shift >= 64)
            {
                throw new InvalidDataException("a number is wider than its type");
            }
            value |= (ulong)(next & 0x7f) << shift;
            if ((next & 0x80) == 0)
            {
                return value;
            }
            shift += 7;
        }
    }
}
