// The film: each frame as a grid of cells, each light or dark.

using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace Solium.Wm;

/// <summary>
/// What <c>frames.py</c> writes: a few lines saying the grid and the frames
/// a second, and then a line a frame, a <c>1</c> for each light cell and a
/// <c>0</c> for each dark one, row by row.
/// </summary>
public sealed class Film
{
    public int Columns { get; private init; }
    public int Rows { get; private init; }
    public double FramesASecond { get; private init; }

    /// <summary>How many cells a frame has: how many terminals of each theme it takes.</summary>
    public int Cells => Columns * Rows;

    /// <summary>Each frame: whether each cell is light, row by row.</summary>
    public List<bool[]> Frames { get; } = new();

    public static Film Load(string path)
    {
        using var reader = new StreamReader(path);
        int columns = 0, rows = 0;
        double rate = 0;
        string? line;
        // The header ends at the line that says how many frames follow.
        while ((line = reader.ReadLine()) is not null)
        {
            var words = line.Split(' ', System.StringSplitOptions.RemoveEmptyEntries);
            if (words.Length == 0)
            {
                continue;
            }
            if (words[0] == "grid")
            {
                columns = int.Parse(words[1], CultureInfo.InvariantCulture);
                rows = int.Parse(words[2], CultureInfo.InvariantCulture);
            }
            else if (words[0] == "fps")
            {
                rate = double.Parse(words[1], CultureInfo.InvariantCulture);
            }
            else if (words[0] == "frames")
            {
                break;
            }
        }
        if (columns <= 0 || rows <= 0 || rate <= 0)
        {
            throw new InvalidDataException($"{path} is not a film frames.py wrote");
        }
        var film = new Film { Columns = columns, Rows = rows, FramesASecond = rate };
        while ((line = reader.ReadLine()) is not null)
        {
            if (line.Length != film.Cells)
            {
                continue;
            }
            var light = new bool[film.Cells];
            for (var cell = 0; cell < light.Length; cell++)
            {
                light[cell] = line[cell] == '1';
            }
            film.Frames.Add(light);
        }
        if (film.Frames.Count == 0)
        {
            throw new InvalidDataException($"{path} has no frames");
        }
        return film;
    }
}
