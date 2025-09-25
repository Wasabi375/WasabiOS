# histogram

A collection of histogram data structures which enable counting of occurrences
of values and reporting on the distribution of observations.

The implementations in this crate store counts for quantized value ranges using
a fast indexing algorithm that maps values into either a linear range or a
logarithmic range with linear subdivisions. This is similar to HDRHistogram but
the indexing algorithm is modified to make increments faster.

There are several implementations which target difference use-cases. See the
documentation for details.

## Documentation

See the [API Documentation] on docs.rs

## Support

Create a [new issue](https://github.com/pelikan-io/rustcommon/issues/new) on GitHub.

## Authors

* Brian Martin <brian@pelikan.io>

A full list of [contributors] can be found on [GitHub].

## License & Origin

Histogram is based on the histogram package that is part of [rustcommons] fround on 
[GitHub]. It is licesenced as both MIT and Apache License Version 2.0.
See the provided license files.

[API Documentation]: https://docs.rs/histogram/latest/histogram
[contributors]: https://github.com/pelikan-io/rustcommon/graphs/contributors?type=a
[GitHub]: https://github.com/pelikan-io/rustcommon
[rustcommons]: https://crates.io/crates/histogram
