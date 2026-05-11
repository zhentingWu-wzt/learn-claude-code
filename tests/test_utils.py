"""Unit tests for mypackage.utils."""

import unittest

from mypackage.utils import add, capitalize_words, flatten_list


class TestAdd(unittest.TestCase):
    """Tests for the add function."""

    def test_add_integers(self) -> None:
        """Adding two integers should return the correct sum."""
        self.assertEqual(add(3, 4), 7)

    def test_add_floats(self) -> None:
        """Adding two floats should return the correct sum."""
        self.assertAlmostEqual(add(2.5, 3.1), 5.6)

    def test_add_negative(self) -> None:
        """Adding a positive and a negative number should work."""
        self.assertEqual(add(10, -3), 7)

    def test_add_zero(self) -> None:
        """Adding zero should return the same number."""
        self.assertEqual(add(0, 42), 42)
        self.assertEqual(add(42, 0), 42)


class TestCapitalizeWords(unittest.TestCase):
    """Tests for the capitalize_words function."""

    def test_simple_sentence(self) -> None:
        """A standard sentence should have each word capitalized."""
        self.assertEqual(capitalize_words("hello world"), "Hello World")

    def test_already_capitalized(self) -> None:
        """Already-capitalized words should stay the same."""
        self.assertEqual(capitalize_words("Hello World"), "Hello World")

    def test_mixed_case(self) -> None:
        """Mixed-case input should be normalized."""
        self.assertEqual(capitalize_words("hELLO wORLD"), "Hello World")

    def test_single_word(self) -> None:
        """A single word should just be capitalized."""
        self.assertEqual(capitalize_words("python"), "Python")

    def test_empty_string(self) -> None:
        """An empty string should remain empty."""
        self.assertEqual(capitalize_words(""), "")


class TestFlattenList(unittest.TestCase):
    """Tests for the flatten_list function."""

    def test_integer_lists(self) -> None:
        """Flattening lists of integers should work."""
        self.assertEqual(flatten_list([[1, 2], [3, 4]]), [1, 2, 3, 4])

    def test_string_lists(self) -> None:
        """Flattening lists of strings should work."""
        self.assertEqual(
            flatten_list([["a", "b"], ["c"]]), ["a", "b", "c"]
        )

    def test_empty_sublists(self) -> None:
        """Empty sublists should be skipped gracefully."""
        self.assertEqual(flatten_list([[1, 2], [], [3]]), [1, 2, 3])

    def test_empty_input(self) -> None:
        """An empty list should return an empty list."""
        self.assertEqual(flatten_list([]), [])

    def test_mixed_types(self) -> None:
        """Sublists with mixed types should be preserved."""
        self.assertEqual(
            flatten_list([[1, "a"], [True, 3.14]]),
            [1, "a", True, 3.14],
        )


if __name__ == "__main__":
    unittest.main()
