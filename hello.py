"""A simple greeting module.

This module provides a friendly greeting function that can be used
to say hello either from the command line or programmatically.
"""


def greet(name: str = "World") -> str:
    """Return a greeting string for the given name.

    Args:
        name: The name of the person or entity to greet.
            Defaults to "World".

    Returns:
        A formatted greeting string.
    """
    return f"Hello, {name}!"


def main() -> None:
    """Run the main entry point of the application."""
    print(greet())


if __name__ == "__main__":
    main()
