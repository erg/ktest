use std::fmt::Write;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use regex::Regex;
extern crate cgi;
extern crate querystring;

mod lib;
use lib::*;

const COMMIT_FILTER:    &str = include_str!("../commit-filter");
const STYLESHEET:       &str = "bootstrap.min.css";

fn read_file(f: &Path) -> Option<String> {
    let mut ret = String::new();
    let mut file = File::open(f).ok()?;
    file.read_to_string(&mut ret).ok()?;
    Some(ret)
}

#[derive(PartialEq)]
enum TestStatus {
    InProgress,
    Passed,
    Failed,
    NotRun,
    NotStarted,
    Unknown,
}

impl TestStatus {
    fn from_str(status: &str) -> TestStatus {
        if status.is_empty() {
            TestStatus::InProgress
        } else if status.contains("IN PROGRESS") {
            TestStatus::InProgress
        } else if status.contains("PASSED") {
            TestStatus::Passed
        } else if status.contains("FAILED") {
            TestStatus::Failed
        } else if status.contains("NOTRUN") {
            TestStatus::NotRun
        } else if status.contains("NOT STARTED") {
            TestStatus::NotStarted
        } else {
            TestStatus::Unknown
        }
    }

    fn to_str(&self) -> &'static str {
        match self {
            TestStatus::InProgress  => "In progress",
            TestStatus::Passed      => "Passed",
            TestStatus::Failed      => "Failed",
            TestStatus::NotRun      => "Not run",
            TestStatus::NotStarted  => "Not started",
            TestStatus::Unknown     => "Unknown",
        }
    }

    fn table_class(&self) -> &'static str {
        match self {
            TestStatus::InProgress  => "table-secondary",
            TestStatus::Passed      => "table-success",
            TestStatus::Failed      => "table-danger",
            TestStatus::NotRun      => "table-secondary",
            TestStatus::NotStarted  => "table-secondary",
            TestStatus::Unknown     => "table-secondary",
        }
    }
}

struct TestResult {
    name:           String,
    status:         TestStatus,
    duration:       usize,
}

fn read_test_result(testdir: &std::fs::DirEntry) -> Option<TestResult> {
    Some(TestResult {
        name:       testdir.file_name().into_string().unwrap(),
        status:     TestStatus::from_str(&read_file(&testdir.path().join("status"))?),
        duration:   read_file(&testdir.path().join("duration")).unwrap_or("0".to_string()).parse().unwrap_or(0),
    })
}

struct Ci {
    ktestrc:            Ktestrc,
    repo:               git2::Repository,
    stylesheet:         String,
    script_name:        String,

    branch:             Option<String>,
    commit:             Option<String>,
    tests_matching:     Regex,
}

fn __commit_get_results(ci: &Ci, commit_id: &String) -> Vec<TestResult> {
    let r = ci.ktestrc.ci_output_dir.join(commit_id).read_dir();

    if let Ok(r) = r {
        let mut dirents: Vec<_> = r.filter_map(|i| i.ok())
            .filter(|i| ci.tests_matching.is_match(&i.file_name().to_string_lossy()))
            .collect();

        dirents.sort_by_key(|x| x.file_name());

        dirents.iter().map(|x| read_test_result(x)).filter_map(|i| i).collect()
    } else {
        Vec::new()
    }
}

struct CommitResults {
    id:             String,
    message:        String,
    tests:          Vec<TestResult>
}

fn commit_get_results(ci: &Ci, commit: &git2::Commit) -> CommitResults {
    let id = commit.id().to_string();
    let tests = __commit_get_results(ci, &id);

    CommitResults {
        id:         id,
        message:    commit.message().unwrap().to_string(),
        tests:      tests,
    }
}

fn branch_get_results(ci: &Ci) -> Result<Vec<CommitResults>, String> {
    let mut nr_empty = 0;
    let mut ret: Vec<CommitResults> = Vec::new();

    let branch = ci.branch.as_ref().unwrap();
    let mut walk = ci.repo.revwalk().unwrap();

    let reference = git_get_commit(&ci.repo, branch.clone());
    if reference.is_err() {
        /* XXX: return a 404 */
        return Err(format!("commit not found"));
    }
    let reference = reference.unwrap();

    if let Err(e) = walk.push(reference.id()) {
        return Err(format!("Error walking {}: {}", branch, e));
    }

    for commit in walk
            .filter_map(|i| i.ok())
            .filter_map(|i| ci.repo.find_commit(i).ok()) {
        let r = commit_get_results(ci, &commit);

        if !r.tests.is_empty() {
            nr_empty = 0;
        } else {
            nr_empty += 1;
            if nr_empty > 100 {
                break;
            }
        }

        ret.push(r);
    }

    
    while !ret.is_empty() && ret[ret.len() - 1].tests.is_empty() {
        ret.pop();
    }

    Ok(ret)
}

fn ci_log(ci: &Ci) -> cgi::Response {
    let mut out = String::new();
    let branch = ci.branch.as_ref().unwrap();

    let commits = branch_get_results(ci);
    if let Err(e) = commits {
        return error_response(e);
    }

    let commits = commits.unwrap();

    let mut multiple_test_view = false;
    for r in &commits {
        if r.tests.len() > 1 {
            multiple_test_view = true;
        }
    }

    writeln!(&mut out, "<!DOCTYPE HTML>").unwrap();
    writeln!(&mut out, "<html><head><title>{}</title></head>", branch).unwrap();
    writeln!(&mut out, "<link href=\"{}\" rel=\"stylesheet\">", ci.stylesheet).unwrap();

    writeln!(&mut out, "<body>").unwrap();
    writeln!(&mut out, "<div class=\"container\">").unwrap();
    writeln!(&mut out, "<table class=\"table\">").unwrap();


    if multiple_test_view {
        writeln!(&mut out, "<tr>").unwrap();
        writeln!(&mut out, "<th> Commit      </th>").unwrap();
        writeln!(&mut out, "<th> Description </th>").unwrap();
        writeln!(&mut out, "<th> Passed      </th>").unwrap();
        writeln!(&mut out, "<th> Failed      </th>").unwrap();
        writeln!(&mut out, "<th> Not started </th>").unwrap();
        writeln!(&mut out, "<th> Not run     </th>").unwrap();
        writeln!(&mut out, "<th> In progress </th>").unwrap();
        writeln!(&mut out, "<th> Unknown     </th>").unwrap();
        writeln!(&mut out, "<th> Total       </th>").unwrap();
        writeln!(&mut out, "<th> Duration    </th>").unwrap();
        writeln!(&mut out, "</tr>").unwrap();

        let mut nr_empty = 0;
        for r in &commits {
            if !r.tests.is_empty() {
                if nr_empty != 0 {
                    writeln!(&mut out, "<tr> <td> ({} untested commits) </td> </tr>", nr_empty).unwrap();
                    nr_empty = 0;
                }

                fn count(r: &Vec<TestResult>, t: TestStatus) -> usize {
                    r.iter().filter(|x| x.status == t).count()
                }

                let subject_len = r.message.find('\n').unwrap_or(r.message.len());

                let duration: usize = r.tests.iter().map(|x| x.duration).sum();

                writeln!(&mut out, "<tr>").unwrap();
                writeln!(&mut out, "<td> <a href=\"{}?branch={}&commit={}\">{}</a> </td>",
                         ci.script_name, branch,
                         r.id, &r.id.as_str()[..14]).unwrap();
                writeln!(&mut out, "<td> {} </td>", &r.message[..subject_len]).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::Passed)).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::Failed)).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::NotStarted)).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::NotRun)).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::InProgress)).unwrap();
                writeln!(&mut out, "<td> {} </td>", count(&r.tests, TestStatus::Unknown)).unwrap();
                writeln!(&mut out, "<td> {} </td>", r.tests.len()).unwrap();
                writeln!(&mut out, "<td> {}s </td>", duration).unwrap();
                writeln!(&mut out, "</tr>").unwrap();
            } else {
                nr_empty += 1;
            }
        }
    } else {
        writeln!(&mut out, "<tr>").unwrap();
        writeln!(&mut out, "<th> Commit      </th>").unwrap();
        writeln!(&mut out, "<th> Description </th>").unwrap();
        writeln!(&mut out, "<th> Status      </th>").unwrap();
        writeln!(&mut out, "<th> Duration    </th>").unwrap();
        writeln!(&mut out, "</tr>").unwrap();

        let mut nr_empty = 0;
        for r in &commits {
            if !r.tests.is_empty() {
                if nr_empty != 0 {
                    writeln!(&mut out, "<tr> <td> ({} untested commits) </td> </tr>", nr_empty).unwrap();
                    nr_empty = 0;
                }

                let subject_len = r.message.find('\n').unwrap_or(r.message.len());
                let t = &r.tests[0];

                writeln!(&mut out, "<tr class={}>", t.status.table_class()).unwrap();
                writeln!(&mut out, "<td> <a href=\"{}?branch={}&commit={}\">{}</a> </td>",
                         ci.script_name, branch,
                         r.id, &r.id.as_str()[..14]).unwrap();
                writeln!(&mut out, "<td> {} </td>", &r.message[..subject_len]).unwrap();
                writeln!(&mut out, "<td> {} </td>", t.status.to_str()).unwrap();
                writeln!(&mut out, "<td> {}s </td>", t.duration).unwrap();
                writeln!(&mut out, "<td> <a href=c/{}/{}/log.br>        log                 </a> </td>", &r.id, t.name).unwrap();
                writeln!(&mut out, "<td> <a href=c/{}/{}/full_log.br>   full log            </a> </td>", &r.id, t.name).unwrap();
                writeln!(&mut out, "<td> <a href=c/{}/{}>		        output directory    </a> </td>", &r.id, t.name).unwrap();
                writeln!(&mut out, "</tr>").unwrap();
            } else {
                nr_empty += 1;
            }
        }

    }

    writeln!(&mut out, "</table>").unwrap();
    writeln!(&mut out, "</div>").unwrap();
    writeln!(&mut out, "</body>").unwrap();
    writeln!(&mut out, "</html>").unwrap();
    cgi::html_response(200, out)
}

fn ci_commit(ci: &Ci) -> cgi::Response {
    let commit_id = ci.commit.as_ref().unwrap();
    let mut out = String::new();
    let commit = git_get_commit(&ci.repo, commit_id.clone());
    if commit.is_err() {
        /* XXX: return a 404 */
        return error_response(format!("commit not found"));
    }
    let commit = commit.unwrap();

    let message = commit.message().unwrap();
    let subject_len = message.find('\n').unwrap_or(message.len());

    writeln!(&mut out, "<!DOCTYPE HTML>").unwrap();
    writeln!(&mut out, "<html><head><title>{}</title></head>", &message[..subject_len]).unwrap();
    writeln!(&mut out, "<link href=\"{}\" rel=\"stylesheet\">", ci.stylesheet).unwrap();

    writeln!(&mut out, "<body>").unwrap();
    writeln!(&mut out, "<div class=\"container\">").unwrap();

    writeln!(&mut out, "<h3><th>{}</th></h3>", &message[..subject_len]).unwrap();

    out.push_str(COMMIT_FILTER);

    writeln!(&mut out, "<table class=\"table\">").unwrap();

    for result in __commit_get_results(ci, &commit_id) {
        writeln!(&mut out, "<tr class={}>", result.status.table_class()).unwrap();
        writeln!(&mut out, "<td> {} </td>", result.name).unwrap();
        writeln!(&mut out, "<td> {} </td>", result.status.to_str()).unwrap();
        writeln!(&mut out, "<td> {}s </td>", result.duration).unwrap();
        writeln!(&mut out, "<td> <a href=c/{}/{}/log.br>        log                 </a> </td>", &commit_id, result.name).unwrap();
        writeln!(&mut out, "<td> <a href=c/{}/{}/full_log.br>   full log            </a> </td>", &commit_id, result.name).unwrap();
        writeln!(&mut out, "<td> <a href=c/{}/{}>		        output directory    </a> </td>", &commit_id, result.name).unwrap();

        if let Some(branch) = &ci.branch {
            writeln!(&mut out, "<td> <a href={}?branch={}&test=^{}$> git log        </a> </td>",
                     ci.script_name, &branch, result.name).unwrap();
        }

        writeln!(&mut out, "</tr>").unwrap();
    }

    writeln!(&mut out, "</table>").unwrap();
    writeln!(&mut out, "</div>").unwrap();
    writeln!(&mut out, "</body>").unwrap();
    writeln!(&mut out, "</html>").unwrap();
    cgi::html_response(200, out)
}

fn ci_list_branches(ci: &Ci) -> cgi::Response {
    let mut out = String::new();

    writeln!(&mut out, "<!DOCTYPE HTML>").unwrap();
    writeln!(&mut out, "<html><head><title>CI branch list</title></head>").unwrap();
    writeln!(&mut out, "<link href=\"{}\" rel=\"stylesheet\">", ci.stylesheet).unwrap();

    writeln!(&mut out, "<body>").unwrap();
    writeln!(&mut out, "<table class=\"table\">").unwrap();

    let lines = read_lines(&ci.ktestrc.ci_branches_to_test);
    if let Err(e) = lines {
        return error_response(format!("error opening ci_branches_to_test {:?}: {}", ci.ktestrc.ci_branches_to_test, e));
    }
    let lines = lines.unwrap();

    let branches: std::collections::HashSet<_> = lines
        .filter_map(|i| i.ok())
        .map(|i| if let Some(w) = i.split_whitespace().nth(0) { Some(String::from(w)) } else { None })
        .filter_map(|i| i)
        .collect();

    let mut branches: Vec<_> = branches.iter().collect();
    branches.sort();

    for b in branches {
        writeln!(&mut out, "<tr> <th> <a href={}?branch={}>{}</a> </th> </tr>", ci.script_name, b, b).unwrap();
    }

    writeln!(&mut out, "</table>").unwrap();
    writeln!(&mut out, "</div>").unwrap();
    writeln!(&mut out, "</body>").unwrap();
    writeln!(&mut out, "</html>").unwrap();
    cgi::html_response(200, out)
}

fn cgi_header_get(request: &cgi::Request, name: &str) -> String {
    request.headers().get(name)
        .map(|x| x.to_str())
        .transpose().ok().flatten()
        .map(|x| x.to_string())
        .unwrap_or(String::new())
}

fn error_response(msg: String) -> cgi::Response {
    let mut out = String::new();
    writeln!(&mut out, "{}", msg).unwrap();
    let env: Vec<_> = std::env::vars().collect();
    writeln!(&mut out, "env: {:?}", env).unwrap();
    cgi::text_response(200, out)
}

cgi::cgi_main! {|request: cgi::Request| -> cgi::Response {
    let ktestrc = ktestrc_read();
    if let Err(e) = ktestrc {
        return error_response(format!("could not read config; {}", e));
    }
    let ktestrc = ktestrc.unwrap();

    if !ktestrc.ci_output_dir.exists() {
        return error_response(format!("required file missing: JOBSERVER_OUTPUT_DIR (got {:?})",
                                      ktestrc.ci_output_dir));
    }

    let repo = git2::Repository::open(&ktestrc.ci_linux_repo);
    if let Err(e) = repo {
        return error_response(format!("error opening repository {:?}: {}", ktestrc.ci_linux_repo, e));
    }
    let repo = repo.unwrap();

    let query = cgi_header_get(&request, "x-cgi-query-string");
    let query: std::collections::HashMap<_, _> =
        querystring::querify(&query).into_iter().collect();

    let tests_matching = query.get("test").unwrap_or(&"");

    let ci = Ci {
        ktestrc:            ktestrc,
        repo:               repo,
        stylesheet:         String::from(STYLESHEET),
        script_name:        cgi_header_get(&request, "x-cgi-script-name"),

        branch:             query.get("branch").map(|x| x.to_string()),
        commit:             query.get("commit").map(|x| x.to_string()),
        tests_matching:     Regex::new(tests_matching).unwrap_or(Regex::new("").unwrap()),
    };

    if ci.commit.is_some() {
        ci_commit(&ci)
    } else if ci.branch.is_some() {
        ci_log(&ci)
    } else {
        ci_list_branches(&ci)
    }
} }