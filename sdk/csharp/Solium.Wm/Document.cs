// A settings document, from a value of the language's own.

using System;
using System.Collections.Generic;
using System.Text.Json;

namespace Solium.Wm;

/// <summary>
/// The protocol carries a settings section as a document: a list of nodes,
/// the first the root, a table naming its children by their place in the
/// list. This writes one from anything that reads as JSON: an anonymous
/// object, a dictionary, a list, a number, text.
/// </summary>
public static class Document
{
    public static List<DocNode> From(object? value)
    {
        var nodes = new List<DocNode>();
        Add(JsonSerializer.SerializeToElement(value), nodes);
        return nodes;
    }

    /// <summary>Add a value, and whatever it holds after it: its place in the list.</summary>
    private static uint Add(JsonElement value, List<DocNode> nodes)
    {
        var at = (uint)nodes.Count;
        switch (value.ValueKind)
        {
            case JsonValueKind.Object:
            {
                // Its place is taken first, so its children come after it.
                var table = new DocTable();
                nodes.Add(new DocNode { Table = table });
                foreach (var field in value.EnumerateObject())
                {
                    table.Fields.Add(new DocField { Name = field.Name, Node = Add(field.Value, nodes) });
                }
                break;
            }
            case JsonValueKind.Array:
            {
                var table = new DocTable();
                nodes.Add(new DocNode { Table = table });
                foreach (var item in value.EnumerateArray())
                {
                    table.List.Add(Add(item, nodes));
                }
                break;
            }
            case JsonValueKind.String:
                nodes.Add(new DocNode { Text = value.GetString() ?? "" });
                break;
            case JsonValueKind.Number:
                nodes.Add(value.TryGetInt64(out var whole)
                    ? new DocNode { Integer = whole }
                    : new DocNode { Number = value.GetDouble() });
                break;
            case JsonValueKind.True:
            case JsonValueKind.False:
                nodes.Add(new DocNode { Boolean = value.GetBoolean() });
                break;
            case JsonValueKind.Null:
            case JsonValueKind.Undefined:
                nodes.Add(new DocNode { Nil = new Unit() });
                break;
            default:
                throw new ArgumentException($"a {value.ValueKind} is not a value a document carries");
        }
        return at;
    }
}
