use crate::net::{self, BackendMeta, Session, BACKENDS};
use evscode::R;
use futures::future::join_all;
use std::sync::Arc;
use unijudge::{boxed::BoxedContestDetails, Backend};

pub async fn fetch_contests() -> Vec<(Arc<net::Session>, BoxedContestDetails, &'static BackendMeta)> {
	let _status = crate::STATUS.push("Fetching contests");
	let domains = BACKENDS
		.iter()
		.filter(|backend| backend.backend.supports_contests())
		.flat_map(|backend| backend.backend.accepted_domains().iter().map(move |domain| (*domain, backend)))
		.collect::<Vec<_>>();
	let _status = crate::STATUS.push_silence();
	join_all(domains.iter().map(|(domain, backend)| {
		async move {
			(
				domain,
				try {
					let _status = crate::STATUS.push(format!("Connecting {}", domain));
					let sess = Arc::new(Session::connect(domain, backend.backend).await?);
					let _status = crate::STATUS.push(format!("Fetching {}", domain));
					let contests = sess.run(|backend, sess| backend.contests(sess)).await?;
					(sess, contests, *backend)
				},
			)
		}
	}))
	.await
	.into_iter()
	.flat_map(|(domain, resp): (_, R<_>)| match resp {
		Ok((sess, contests, backend)) => contests.into_iter().map(move |contest| (sess.clone(), contest, backend)).collect(),
		Err(e) => {
			e.context(format!("failed to fetch {} contests", domain)).warning().emit();
			Vec::new()
		},
	})
	.collect()
}
