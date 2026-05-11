"""Utility functions for the mypackage module.

This module provides general-purpose utility functions including
arithmetic operations, string manipulation, and data structure helpers.
"""

from typing import Any


def add(a: float, b: float) -> float:
    """Return the sum of two numbers.

    Args:
        a: The first addend.
        b: The second addend.

    Returns:
        The sum of a and b.
    """
    return a + b


def capitalize_words(text: str) -> str:
    """Capitalize the first letter of each word in a string.

    Args:
        text: The input string to transform.

    Returns:
        A new string with each word capitalized.

    Example:
        >>> capitalize_words("hello world")
        'Hello World'
    """
    return " ".join(word.capitalize() for word in text.split())


def flatten_list(nested: list[list[Any]]) -> list[Any]:
    """Flatten a list of lists into a single flat list.

    Args:
        nested: A list containing sublists.

    Returns:
        A new flat list with all elements from the sublists concatenated.

    Example:
        >>> flatten_list([[1, 2], [3, 4]])
        [1, 2, 3, 4]
    """
    result: list[Any] = []
    for sublist in nested:
        result.extend(sublist)
    return result


def main() -> None:
    """Demonstrate the utility functions."""
    print("add(3, 4):", add(3, 4))
    print('capitalize_words("hello world"):', capitalize_words("hello world"))
    print("flatten_list([[1, 2], [3, 4]]):", flatten_list([[1, 2], [3, 4]]))
    print("flatten_list([['first', 'a'], ['second', 'b']]):",
          flatten_list([["first", "a"], ["second", "b"]]))


if __name__ == "__main__":
    main()
